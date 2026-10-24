# lokal — Specification (v2, macOS)

> **Status.** This is the design specification lokal v0.1 was built from, and it is still the source of truth
> for behaviour: when the code and this document disagree, one of them has a bug. It was written to be executed by
> a coding agent, so it also contains build milestones (§15) and on-machine experiments (§12). Deviations found while
> building are recorded, with evidence, in the Decisions section of [`history/PROGRESS.md`](history/PROGRESS.md).
>
> Some examples predate the switch of the default TLD from `.local` to `.lokal` and still show `.local` names
> (§4.2, §4.5, §4.8). Both TLDs work; new projects get `.lokal`.
>
> For a shorter tour of the code, read [`ARCHITECTURE.md`](ARCHITECTURE.md). For the live test, read
> [`TESTING.md`](TESTING.md).

## 1. Goal

A single Rust CLI binary that gives every service in a local development setup its own `.lokal` (or `.local`) domain with valid HTTPS. The developer sets it up once and never thinks about certificates again.

```console
$ lokal setup                        # one-time onboarding; asks for your password once
$ lokal project create myproject     # claims myproject.lokal
$ lokal service add api 4000         # https://api.myproject.lokal -> localhost:4000  (ready in 40ms)
$ lokal service add web 3000         # https://web.myproject.lokal -> localhost:3000
$ curl https://api.myproject.lokal   # works, no -k, no --cacert
```

**The product has two phases:**

1. **Onboarding (`lokal setup`), once per Mac.** This is the only time anything privileged or interactive happens. It is allowed to be slow-ish and chatty, and to ask for a password.
2. **Daily use, which must be fast and silent.** Creating a project or adding, changing or removing a service needs no sudo, no prompts and no restarts. The URL works the moment the command returns.

### 1.1 Scope

| | |
|---|---|
| Platform | **macOS only**, version 14 Sonoma and later, Apple Silicon (Intel best-effort) |
| Other platforms | Not built now. The code is structured so Linux and Windows can be added later without touching the core (§10, Appendix A) |
| TLD | `.lokal` (default; built-in DNS server, §7.2) and `.local` (mDNS, §7.1). `.lokal` is not a reserved TLD: doctor warns if it is ever delegated (§11) |

**Non-goals (v1):** Linux and Windows builds, public tunnels, a GUI or menu-bar app, Docker integration, LAN or phone access, upstream HTTPS, multiple OS users on one machine.

## 2. Glossary

- **Project**: a named group of services that owns one **apex domain** (e.g. `myproject.lokal`, or `myproject.local`).
- **Service**: a **subdomain** of the apex (e.g. `api`) routed to an **upstream** (`host:port`, e.g. `localhost:4000`). The special subdomain `@` routes the apex itself.
- **FQDN**: the full hostname of a service, e.g. `api.myproject.lokal`.
- **Daemon**: the background process (`lokal daemon run`) that runs as a per-user LaunchAgent. It terminates TLS, proxies requests and publishes names.
- **CA**: the per-Mac certificate authority that `lokal setup` creates and macOS trusts.
- **Ready**: a service is ready when its route is live, its name resolves, and its leaf certificate has been issued.

## 3. Architecture

### 3.1 What onboarding buys us

Each piece is designed so that a one-time step during onboarding removes every runtime privilege:

| Problem | One-time onboarding step | Runtime result |
|---|---|---|
| Browsers must trust our certificates | Create a CA (as the user) and trust it in the System keychain. **This is the only `sudo` command.** | Leaf certs are issued in memory on demand; nothing ever touches the keychain again |
| Ports 443 and 80 are privileged | Install a LaunchAgent whose plist declares the 443/80 sockets. **launchd binds them** and hands them to the daemon (socket activation, the approach puma-dev uses) | The daemon runs as the user and never binds privileged ports itself. Listeners survive daemon restarts |
| `api.myproject.local` must resolve | Nothing | The daemon registers loopback A/AAAA records with the system mDNSResponder on the **LocalOnly** interface. Unprivileged, instant, and invisible to the LAN |
| `api.myproject.lokal` must resolve | Install `/etc/resolver/lokal` (same sudo batch as the trust step) pointing `.lokal` at `127.0.0.1:53535` | The daemon's built-in DNS server answers on a launchd-owned UDP socket. mDNSResponder refuses LocalOnly records outside `.local` (`kDNSServiceErr_BadParam`, measured), hence the resolver file |

The socket-activation and mDNS designs depend on macOS behaviour that must be confirmed on a real Mac. These are the **VERIFY items in §12**, each with a fallback.

### 3.2 Components

One binary, two modes.

- **CLI mode** (`lokal <command>`) edits the state file, talks to the daemon over a Unix socket, and runs onboarding and uninstall.
- **Daemon mode** (`lokal daemon run`, hidden from help) contains:
  - **Listeners:** sockets from launchd (production), or bound directly on high ports (tests).
  - **Cert issuer:** a rustls `ResolvesServerCert` that issues CA-signed leaves per SNI and pre-warms them when a service is added.
  - **Proxy:** routes by Host/SNI to upstreams, including WebSocket, SSE and h2.
  - **Name publisher:** mDNS LocalOnly records through `dns_sd`.
  - **IPC server:** `ping`, `status`, `reload` and `selftest`. `reload` returns only when every affected FQDN is ready.
  - **State watcher:** watches `state.toml` as a backup to IPC `reload`, for hand edits.

```
                       ┌──────────── lokal daemon (user, LaunchAgent) ────────────┐
browser/curl/node ─DNS─▶ mDNSResponder ◀── dns_sd LocalOnly A/AAAA (per FQDN)      │
        │                                                                          │
        └─TLS 127.0.0.1:443 ─▶ launchd-owned socket ─▶ rustls (on-demand leaf) ─▶ proxy ─▶ localhost:4000
                                                                                   │
lokal CLI ── state.toml (atomic write) ── IPC reload (waits for "ready") ──────────┘
```

## 4. CLI

### 4.1 Command tree

```
lokal setup [--yes] [--dry-run]                                   alias: install
lokal uninstall [--purge] [--yes]
lokal doctor [--json]

lokal project create <NAME> [--domain <DOMAIN>]                   alias: project new
lokal project list [--json]                                       alias: project ls
lokal project show [NAME] [--json]
lokal project use <NAME>                                          alias: project switch
lokal project remove <NAME> [--force]                             alias: project rm

lokal service add <SUBDOMAIN> <PORT> [--host <HOST>] [--host-header <preserve|upstream>] [-p <PROJECT>]   alias: service new
lokal service list [-p <PROJECT>] [--json]                        alias: service ls
lokal service update <SUBDOMAIN> [--port <PORT>] [--host <HOST>] [--host-header <..>] [-p <PROJECT>]     alias: service set
lokal service remove <SUBDOMAIN> [-p <PROJECT>]                   alias: service rm

lokal list [--json]                                               alias: ls

lokal start | stop | restart
lokal status [--json]
lokal logs [-f] [-n <LINES>]
lokal upgrade

lokal env [--shell <sh|fish>]
lokal ca path
lokal open <SUBDOMAIN> [-p <PROJECT>]

lokal help [COMMAND...]
```

Top-level aliases: `p` = `project`, `s` = `service`.

Global flags, accepted anywhere:
- `-h` (short help) and `--help` (long help)
- `-V`, `--version`, which prints `lokal 0.1.0 (<git short hash> <build date>)`
- `--no-color` (also honour the `NO_COLOR` env var, and disable colour when stdout is not a TTY)
- `-v`, `--verbose` (repeatable; prints debug detail to stderr)

Hidden commands (not in help):
- `daemon run`
- `internal *` (test hooks only)

### 4.2 Help output (required)

`lokal --help` and `lokal help` must show **every** available command, including nested ones. Implement this with a custom root help template whose command list is **generated from the clap command tree at runtime**, so it can never drift from the real commands. A test asserts that every non-hidden leaf command appears. Target output:

```
lokal 0.1.0
Local .local domains with valid HTTPS for every service you run in development.

Usage: lokal <COMMAND>

Get started:
  setup                            One-time onboarding: trust lokal's certificate and start the background service
  doctor                           Check every layer and explain anything that's broken
  uninstall                        Undo setup (--purge also deletes projects and the certificate)

Projects:
  project create <NAME>            Create a project and its domain (NAME.local)
  project list                     List all projects
  project show [NAME]              Show a project's domain and services
  project use <NAME>               Set the current project
  project remove <NAME>            Delete a project

Services:
  service add <SUBDOMAIN> <PORT>   Route https://SUBDOMAIN.<project>.local to localhost:PORT
  service list                     List the current project's services
  service update <SUBDOMAIN>       Change a service's port, host or host-header mode
  service remove <SUBDOMAIN>       Remove a service
  list                             Show every project with its services and URLs

Background service:
  start | stop | restart           Control the background service
  status                           Show status, listeners and published names
  logs                             Show logs (-f to follow)
  upgrade                          Make the background service use this lokal binary

Utilities:
  env                              Print env vars so Node, Python, curl etc. trust lokal's certificate
  ca path                          Print the path of the CA certificate
  open <SUBDOMAIN>                 Open a service in your default browser

Options:
  -h, --help       Print help (see more with '--help')
  -V, --version    Print version
      --no-color   Disable coloured output
  -v, --verbose    More detail on stderr

Quick start:
  lokal setup
  lokal project create myproject
  lokal service add api 4000

Run `lokal help <COMMAND>` for details and examples.
```

Each subcommand's `--help` has an **Examples** section.

`lokal` with no arguments:
- **before setup:** prints a short welcome (one sentence on what lokal does, then `Get started: lokal setup`), followed by the root help. Exit 0.
- **after setup:** prints the root help. Exit 0.

### 4.3 Validation rules

All names are trimmed and lowercased before validation.

- **Label** (shared by all the rules below): 1–63 characters from `[a-z0-9-]`, not starting or ending with `-`.
- **Project name**: a single label. Dots are rejected with a hint to use `--domain`.
- **Apex domain**: `<name>.lokal` by default. `--domain` must have ≥2 labels, a total length ≤253, and end in `.lokal` or `.local`.
  - Any other TLD, including `.test` and `.internal` (the CA's name constraints don't cover them, §6.1), is rejected with: `only .lokal and .local are supported (other TLDs could shadow real websites)`.
- **Overlap**: an apex may not equal, contain or sit under another project's apex. For example, `app.local` and `api.app.local` conflict; `myapp.local` and `app.local` don't.
- **Subdomain**: `@`, or one or more labels joined by dots (`v1.api` is allowed). If the user passes the full FQDN (`api.myproject.local`), strip the apex suffix and accept it. The FQDN must be ≤253 characters.
- **Port**: 1–65535. Reject 443 and 80 when the host is loopback, since they would loop back into lokal.
- **Host**: a hostname or IP address. Default `localhost`. Bracketed IPv6 is accepted and stored without brackets. Reject a scheme (`http://`) or an embedded port, with a hint. Print a warning (not an error) if the host is not loopback.
- **Uniqueness**: subdomains are unique within a project. Re-adding an **identical** service (same host, port and host-header mode) succeeds without changing anything and still waits until it is ready (`(already configured, ready in 5ms)`). Adding an existing one with different settings fails with the exact update command, e.g. ``service `api` already exists (localhost:4000); use `lokal service update api --port 5000` ``.
- **Re-running `project create`**: if the project exists and `--domain` is omitted or equal to its domain, it succeeds and makes it the current project (`✓ myproject.local already exists (current project)`). A different `--domain` fails.

### 4.4 Current-project resolution

For `service *`, `project show` and `open`, the project is chosen in this order:

1. `-p/--project`
2. `current_project` in state
3. if exactly one project exists, that project
4. otherwise, fail with: `no project selected: run 'lokal project use <NAME>' or pass -p <NAME>`

`project create` sets the new project as current. `project remove` clears `current_project` if it pointed at the removed project.

### 4.5 Command behaviour and output

These rules apply to every command:
- Human output goes to stdout. Errors go to stderr as `error: <message>`, optionally followed by `hint: <suggestion>`. The CLI never prompts, except in `setup` and `uninstall`.

**The fast path.** Every mutating command (`project create/remove`, `service add/update/remove`):
1. Writes state atomically.
2. Sends IPC `reload` and **waits until the daemon reports every affected FQDN ready**: route installed, name registered, and leaf cert issued. For removals, ready means the route is gone and the name is deregistered.
3. Prints the result **with the measured time**.

The command returns only once the URL is usable. Scripts and the live tests rely on this: there must be no "wait a second and retry".

```console
$ lokal project create myproject
✓ Created myproject.local (current project)
  Next: lokal service add api 4000

$ lokal service add api 4000
✓ api.myproject.local → localhost:4000  (ready in 38ms)
  https://api.myproject.local

$ lokal service add web 3000
✓ web.myproject.local → localhost:3000  (ready in 31ms)
  https://web.myproject.local
  note: nothing is listening on localhost:3000 yet

$ lokal list
myproject  myproject.local  (current)
  api    https://api.myproject.local    → localhost:4000
  web    https://web.myproject.local    → localhost:3000
shop       shop.local
  (no services)
```

The upstream probe behind the `note:` line is a TCP connect with a 50ms timeout. Its result is informational only.

When things aren't running:
- **Before setup has run**, mutating commands still save state, then print `note: lokal isn't set up on this Mac yet — run 'lokal setup' to make these URLs work`.
- **If setup is done but the daemon is not running**, they print `note: the background service isn't running — start it with 'lokal start'`.
- If the daemon doesn't report ready within 2s, the command exits 1 and names the FQDNs that are not ready and why (the daemon's error string).

Other commands:
- `project remove` fails while the project still has services, unless `--force` is given.
- `open` runs `/usr/bin/open https://<fqdn>`.
- `upgrade` copies the running executable to the daemon binary path (§5.1) atomically, restarts the daemon, waits for a `ping` whose `build_id` matches, and prints `✓ background service now running lokal 0.1.0 (abc1234) — ready in 210ms`. It needs no sudo.
- `env` prints:
  - `NODE_EXTRA_CA_CERTS=<ca.pem>`
  - `SSL_CERT_FILE=<bundle.pem>`
  - `REQUESTS_CA_BUNDLE=<bundle.pem>`
  - `CURL_CA_BUNDLE=<bundle.pem>`
  
  `bundle.pem` is the macOS system roots (`security find-certificate -a -p /System/Library/Keychains/SystemRootCertificates.keychain`) plus the lokal CA. Setup writes it, and `env` regenerates it if it's older than 30 days. `--shell fish` emits `set -gx` syntax.

  `env` and `ca path` hand out the CA as a trust anchor, so they need setup in place (`install.json` and the LaunchAgent), not just the CA files. Otherwise they print the not-set-up error, exit 3 and print nothing on stdout. A plain `uninstall` keeps the CA files, and the `eval "$(lokal env)"` line in a shell profile must then stop trusting the CA.

### 4.6 Onboarding: `lokal setup`

Setup must be idempotent. Re-running it is the repair path, and **it asks for a password only if something privileged is actually missing**.

Rules:
- Refuse to run as root: `run 'lokal setup' as yourself, not with sudo — lokal asks for your password only for the one step that needs it`.
- `--yes` skips the confirmation.
- `--dry-run` prints the plan and the exact privileged command(s), then exits without changing anything.
- `--skip-trust` is hidden and test-only. It does everything except the trust step (see M1 in §15).

Interactive flow on a fresh Mac:

```
$ lokal setup
Welcome to lokal. This one-time setup makes https://<anything>.local work on this Mac.

  1. Create lokal's certificate authority
     Only valid for .lokal and .local names. Its key stays in your user folder.
  2. Trust it in the macOS System keychain                       ← needs your password (once)
  3. Install the background service (LaunchAgent, runs as you)
     macOS will hold ports 443 and 80 on 127.0.0.1 for it. Nothing is reachable from your network.
  4. Run a self-test

Continue? [Y/n] y

  ✓ Created certificate authority                                  12ms
  › Trusting it in the System keychain. Enter your password; macOS may also show a dialog.
    sudo security add-trusted-cert -d -r trustRoot -p ssl -k /Library/Keychains/System.keychain ~/Library/Application Support/lokal/ca/root.pem
Password:
  ✓ Trusted in System keychain                                    3.1s
  ✓ Background service running (127.0.0.1:443, [::1]:443, :80)     180ms
  ✓ Self-test: https://lokal-selftest-3fa9c1.local verified by macOS 44ms

You're all set. Next:
  lokal project create myproject
  lokal service add api 4000
Tip: run 'eval "$(lokal env)"' so Node and Python trust lokal too.
```

Steps:

1. **CA:** create it if missing (§6.1). If it exists, keep it.
2. **Trust:** check trust with `security verify-cert` against a freshly issued test leaf (§6.3). If it already verifies, for TLS only, print `✓ Already trusted` and **do not call sudo**. If it also verifies for every other use (an older lokal trusted it without `-p ssl`), trust it again, which narrows it to TLS; the step reads "Trust it for HTTPS only". Otherwise:
   - Print the exact command, then run `sudo -p "Password for <user> (to trust lokal's certificate): " security add-trusted-cert ...` with the TTY attached.
   - If stdin is not a TTY and sudo would need a password, fail with a hint.
   - When the `LOKAL_FORBID_SUDO=1` test hook is set and sudo would be needed, fail with exit code 4 instead of calling sudo.
3. **Service:**
   - Copy the current executable to the daemon binary path.
   - Write the LaunchAgent plist (§8.1).
   - Run `launchctl bootstrap gui/<uid> <plist>`. If the agent is already loaded and the plist changed, run `bootout` and then `bootstrap`. If it's loaded and unchanged, run `kickstart -k` only when the binary changed.
   - Wait for an IPC `ping` (5s max).
4. **Self-test:** IPC `selftest` makes the daemon temporarily publish `lokal-selftest-<6 hex>.local` (A+AAAA) and serve a built-in `200 lokal ok` on it. The CLI then:
   - resolves it with `getaddrinfo` and checks both 127.0.0.1 and ::1;
   - does a TLS handshake verified by **`rustls-platform-verifier`** (that is, the macOS trust evaluation, not our CA file) and a GET;
   - ends the selftest so the name is removed.
   
   On failure, print the failing layer and the matching `doctor` hint, and exit 1.
5. Write `install.json` (what was done: paths, label, CA SHA-256 fingerprint, version), and regenerate `bundle.pem`.
6. Print the "all set" block.

Other onboarding touches:
- **Firefox:** if Firefox is installed (`/Applications/Firefox.app`), apply the result of VERIFY V5 (§12). If its own trust store doesn't pick up the macOS keychain, append `user_pref("security.enterprise_roots.enabled", true);` to `user.js` in each profile under `~/Library/Application Support/Firefox/Profiles/*`, and say so in the output.
- **Nudges:** any command that needs the daemon while setup has not run prints `lokal isn't set up yet — run 'lokal setup' (one time, takes about 10 seconds)` and exits with code 3.

**`.lokal` additions to setup.** Setup also installs the OS resolver config for `.lokal` (`/etc/resolver/lokal`: `nameserver 127.0.0.1`, `port 53535`, staged in `<base>/resolver/lokal` and copied with `install -m 0644`). If the existing CA's name constraints are not exactly `local` + `lokal` with every IP address excluded (an older CA lacks `lokal`, or also allows `test` and `internal`, or leaves IP addresses unconstrained), setup replaces it: the old certificate moves to `<base>/ca/retired/<sha1>/root.pem` (its key is deleted at once), a new CA is created, and one privileged batch untrusts and deletes the old CA, trusts the new one and installs the resolver file. The daemon is restarted so it loads the new CA. A `root.pem` whose key file is missing is replaced the same way: setup never writes a new CA over a certificate that may still be trusted. If `install.json` records a CA that no file on disk holds any more but the System keychain does, setup finds it as uninstall does (§4.7) and untrusts it in the same batch. The selftest publishes one name per TLD (`lokal-selftest-<hex>.lokal` and `.local`) and checks both (the `.lokal` one only once the resolver is installed). `--skip-trust` skips the resolver too (no sudo at all).

### 4.7 `lokal uninstall`

Reverse of setup, driven by `install.json`:
1. `launchctl bootout gui/<uid>/dev.lokal.daemon`, then remove the plist and the daemon binary.
2. Untrust and remove the CA. This is the only sudo step:
   - `sudo security remove-trusted-cert -d <ca.pem>`
   - `sudo security delete-certificate -Z <sha1> /Library/Keychains/System.keychain`
3. Remove the Firefox `user.js` line if lokal added it (the line is tagged with a trailing comment `// added by lokal`).

Uninstall also removes `/etc/resolver/lokal` and untrusts any retired CA still in the keychain, in the same privileged batch.

Uninstall picks CAs by certificate, not certificate plus key: untrusting needs only the certificate, so a `root.pem` whose key file is gone is untrusted too. If `install.json` records a CA that no certificate on disk matches, uninstall looks for it in the System keychain (read-only, §6.3), matching on the SHA-1 it computes from each lokal CA there. `install.json` is user-writable, so its hash alone never goes into a privileged command. A match is written to `ca/retired/<sha1>/root.pem` and untrusted in the same batch. If the lookup fails, uninstall says so and counts a warning instead of reporting the CA as not in the keychain.

Plain `uninstall` keeps `state.toml` and the CA files, so re-running `setup` restores everything without re-trusting a new CA. It deletes `bundle.pem`, and `env` exports nothing once setup is gone (§4.5). Uninstall ends by telling the user to remove the `eval "$(lokal env)"` line from their shell profile. `--purge` deletes the whole data directory as well.

Uninstall shows the plan and confirms, unless `--yes` is given. It must succeed (with warnings) if pieces are already gone.

### 4.8 JSON output

`--json` output is a stable contract that the live tests use. It is pretty-printed, contains no colour codes, and always has `"schema": 1`.

```jsonc
// lokal list --json
{ "schema": 1, "current_project": "myproject", "projects": [
  { "name": "myproject", "domain": "myproject.local", "services": [
    { "subdomain": "api", "fqdn": "api.myproject.local", "url": "https://api.myproject.local",
      "host": "localhost", "port": 4000, "host_header": "preserve" } ] } ] }

// lokal status --json
{ "schema": 1, "setup_done": true, "running": true, "pid": 1234, "version": "0.1.0",
  "build_id": "0.1.0+abc1234.20261010T120000Z", "uptime_secs": 42,
  "listeners": ["127.0.0.1:443", "[::1]:443", "127.0.0.1:80", "[::1]:80"],
  "listener_source": "launchd",            // or "direct"
  "name_backend": "mdns-local-only",
  "names": [ { "fqdn": "api.myproject.local", "state": "registered" } ],   // or "pending" / "failed: <reason>"
  "certs_cached": 2 }

// lokal doctor --json
{ "schema": 1, "ok": false, "checks": [
  { "id": "ca.trusted", "status": "pass", "detail": "...", "ms": 31 },
  { "id": "upstream.api.myproject.local", "status": "warn", "detail": "nothing listening on localhost:4000" } ] }
```

### 4.9 Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Runtime error, or `doctor` found at least one `fail` |
| 2 | Usage error (clap's default) |
| 3 | Setup has not been run |
| 4 | Sudo would be needed but is forbidden (`LOKAL_FORBID_SUDO=1`, or no TTY) |

## 5. State and files

### 5.1 Locations (macOS)

| What | Path |
|---|---|
| Base dir (state and data) | `~/Library/Application Support/lokal/` |
| State | `<base>/state.toml` |
| CA | `<base>/ca/root.pem` (0644), `<base>/ca/root-key.pem` (0600), `<base>/ca/bundle.pem` |
| Daemon binary | `<base>/bin/lokal` (a copy, managed by `setup`/`upgrade`; user-owned) |
| IPC socket | `<base>/lokal.sock` (0600) |
| Daemon lock | `<base>/daemon.lock` |
| Install record | `<base>/install.json` |
| Logs | `~/Library/Logs/lokal/daemon.log` (rotated daily, keep 3) |
| LaunchAgent | `~/Library/LaunchAgents/dev.lokal.daemon.plist` |

Resolve all paths through one `Paths` struct built with the `directories` crate. `LOKAL_HOME=<dir>` overrides the base dir and the log dir (`<dir>/logs`). Integration tests use this. **No other module may build paths itself.**

### 5.2 `state.toml`

```toml
version = 1
current_project = "myproject"

[projects.myproject]
domain = "myproject.local"
created_at = "2026-10-10T12:00:00Z"

[projects.myproject.services.api]
host = "localhost"
port = 4000
host_header = "preserve"   # or "upstream"

[projects.myproject.services."@"]
host = "localhost"
port = 3000
host_header = "preserve"
```

Rules:
- Every write is atomic: write to `state.toml.tmp`, fsync, then rename.
- Every read-modify-write holds an exclusive lock on `state.toml.lock` (`std::fs::File::lock`).
- Unknown fields must never cause a parse error.
- If `version` is greater than the binary supports, fail with `state file was written by a newer lokal; run a newer lokal`.
- Only the CLI writes the state file. The daemon only reads it.

## 6. Certificates

### 6.1 Root CA

Generated by `rcgen` during `setup`, as the user:

| Field | Value |
|---|---|
| Key | ECDSA P-256 |
| Signature | ecdsa-with-SHA256 |
| Subject CN | `lokal development CA (<user>@<hostname>)` |
| Subject O | `lokal` |
| Validity | 10 years, notBefore = now − 1h |
| basicConstraints | critical, CA:TRUE, pathLen:0 |
| keyUsage | critical, keyCertSign, cRLSign |
| nameConstraints | critical, permitted dNSName: `local`, `lokal` (exactly the TLDs lokal serves); excluded iPAddress: `0.0.0.0/0`, `::/0` |
| Serial | random 128-bit positive |

The name constraints are a security feature: a leaked key cannot impersonate public sites, nor private names under other TLDs such as company `.internal` or `.test` hosts. The CA is trusted system-wide, for every user of the Mac, so the list stays as short as possible. DNS-only constraints leave IP addresses unconstrained (RFC 5280 §4.2.1.10: each name form is constrained separately), so every IPv4 and IPv6 address is excluded too; without that, macOS and Chrome trust a `127.0.0.1` or `192.168.1.1` leaf from the CA (measured 2026-10-17). lokal never issues IP leaves. Their client compatibility is **VERIFY V3**.

**The CA is frozen once a human has trusted it.** Changing any CA field means re-trusting, which needs the human (see `CONTRIBUTING.md`, "Rules for code changes").

### 6.2 Leaf certificates

| Field | Value |
|---|---|
| Key | ECDSA P-256. One key pair is generated per daemon run and shared by all leaves (faster issuance) |
| SAN | exactly one dNSName: the FQDN |
| Subject CN | the FQDN |
| extendedKeyUsage | serverAuth (required by Apple) |
| keyUsage | critical, digitalSignature |
| basicConstraints | CA:FALSE |
| Validity | 90 days, notBefore = now − 1h |

Rules:
- **Pre-warm:** issue the leaf during `reload`, so the first browser handshake never waits on signing.
- In-memory cache keyed by FQDN, re-issued when fewer than 30 days remain. No disk cache. Issuance is around a millisecond, and every restart issues fresh leaves for all configured names in parallel.
- The resolver serves only FQDNs that are currently configured (plus the active selftest name). An unknown SNI, or no SNI, fails the handshake.
- No wildcard certificates.

### 6.3 Trust (macOS)

- **Trust:** `sudo security add-trusted-cert -d -r trustRoot -p ssl -k /Library/Keychains/System.keychain <root.pem>` (during setup only). `-p ssl` limits the trust to TLS server evaluation: without it, macOS trusts the CA for every policy, so a leaf it signs would also pass for S/MIME (email names aren't name-constrained), code signing and TLS client auth (§12 V8). Run over an existing entry, it replaces that entry, so the same command narrows an older lokal's unrestricted trust.
- **Check trust** (setup, doctor, selftest):
  1. issue a throwaway leaf for `lokal-trustcheck.local`;
  2. run `security verify-cert -c <leaf.pem> -r <root.pem> -p ssl -s lokal-trustcheck.local` and `-L` (local only, no network);
  3. confirm with a `rustls-platform-verifier` handshake in the selftest.
- **Check scope** (setup, doctor): verify the same throwaway leaf with `-p basic` instead. It passes only if the CA is trusted for more than TLS (§12 V8); setup then trusts it again with `-p ssl`, and fails with a hint if it still passes (another trust setting, such as "Always Trust" in Keychain Access, covers every use).
- **Find** a lokal CA whose files are gone (setup, uninstall; read-only): `security find-certificate -a -p -c "lokal development CA" /Library/Keychains/System.keychain`, then match on the SHA-1 computed from each certificate. With no match it exits 0 and prints nothing (`crates/testkit/src/bin/verify_keychain_lookup.rs`).
- **Untrust:** see §4.7.

## 7. Name resolution

`.local` names use §7.1 (mDNS); `.lokal` names use §7.2 (built-in DNS server). The daemon splits each reload's name set by TLD. `status.name_backend` is `mdns-local-only+dns`.

### 7.1 `.local`: `mdns-local-only` backend

The daemon registers records with mDNSResponder through the `dns_sd` C API in libSystem. It uses a small hand-written FFI module:
- `DNSServiceCreateConnection`
- `DNSServiceRegisterRecord`
- `DNSServiceRemoveRecord`
- `DNSServiceRefSockFD`
- `DNSServiceProcessResult`
- `DNSServiceRefDeallocate`

Behaviour:
- For each FQDN, register an **A record `127.0.0.1` and an AAAA record `::1`**, on interface `kDNSServiceInterfaceIndexLocalOnly`, with TTL 120.
  - LocalOnly means the names never leave this Mac.
  - The AAAA record matters. Without it, IPv6 lookups for `.local` fall through to multicast and stall for about 5 seconds, which is the classic slow-`.local` bug.
- Use flag `kDNSServiceFlagsUnique` unless VERIFY V2 shows that `Shared` registers faster with the same resolution behaviour. Pick the faster one and record the choice.
- A name is **registered** when its register callback reports `kDNSServiceErr_NoError` for both records. `reload` waits for that callback; it does not use a sleep.
- Drive the connection's socket with `tokio::io::unix::AsyncFd`, and call `DNSServiceProcessResult` when it is readable. Keep all FFI on one dedicated task: `dns_sd` refs are not thread-safe.
- Apply changes by diff: remove stale records, add new ones.
- On a conflict or error callback, mark that FQDN `failed: <reason>` in `status`, and `reload` reports it.
- If the mDNSResponder connection drops (`kDNSServiceErr_ServiceNotRunning`), reconnect with backoff and re-register everything.
- Registrations disappear automatically when the daemon exits.

Targets: a lookup through `getaddrinfo` returns both addresses in under 20ms right after registration, and under 5ms when repeated.

**Fallback** if VERIFY V2 fails: the `hosts-file` backend (Appendix B). It needs an extra privileged step at setup.

### 7.2 `.lokal`: built-in DNS server

- The daemon serves DNS for the zone `lokal` on `127.0.0.1:53535/udp`, a launchd socket (`dns4`, `SockType dgram`), so queries during a restart wait in the socket queue. Tests bind it directly (`daemon run --dns-port <p>`).
- The OS sends `*.lokal` lookups there via `/etc/resolver/lokal`, installed by setup (§4.6).
- **Every name under `.lokal` answers A `127.0.0.1` / AAAA `::1` with TTL 0, configured or not.** Other types get NODATA with an SOA whose TTL and minimum are 0. Names outside the zone get REFUSED.
- Why no negative answers: measured on macOS 27, mDNSResponder caches **any** failed lookup for ~75s (NXDOMAIN, NODATA, SERVFAIL or REFUSED; with or without SOA; SOA minimum 0 ignored). A name looked up before `service add` would stay unresolvable for 75s, breaking "the URL works when the command returns". With a wildcard, nothing is negatively cached; positive TTL 0 is honoured.
- Whether a `.lokal` name is **served** is decided by routes and certificates (§6.2): an unconfigured or removed name resolves to loopback but its TLS handshake fails, and plain HTTP gets the 404 page.
- Readiness: a `.lokal` name is ready as soon as its route and certificate are in place.

## 8. Background service

### 8.1 LaunchAgent with socket activation

`~/Library/LaunchAgents/dev.lokal.daemon.plist`:

```xml
<key>Label</key><string>dev.lokal.daemon</string>
<key>ProgramArguments</key>
<array><string>/Users/<u>/Library/Application Support/lokal/bin/lokal</string><string>daemon</string><string>run</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>            <!-- names are published only while the daemon runs, so it must always run -->
<key>ProcessType</key><string>Interactive</string>
<key>EnvironmentVariables</key><dict><key>RUST_LOG</key><string>info</string></dict>
<key>StandardOutPath</key><string>/Users/<u>/Library/Logs/lokal/launchd.out.log</string>
<key>StandardErrorPath</key><string>/Users/<u>/Library/Logs/lokal/launchd.err.log</string>
<key>Sockets</key>
<dict>
  <key>https4</key><dict><key>SockNodeName</key><string>127.0.0.1</string><key>SockServiceName</key><string>443</string><key>SockFamily</key><string>IPv4</string></dict>
  <key>https6</key><dict><key>SockNodeName</key><string>::1</string><key>SockServiceName</key><string>443</string><key>SockFamily</key><string>IPv6</string></dict>
  <key>http4</key> <dict><key>SockNodeName</key><string>127.0.0.1</string><key>SockServiceName</key><string>80</string><key>SockFamily</key><string>IPv4</string></dict>
  <key>http6</key> <dict><key>SockNodeName</key><string>::1</string><key>SockServiceName</key><string>80</string><key>SockFamily</key><string>IPv6</string></dict>
  <key>dns4</key>  <dict><key>SockNodeName</key><string>127.0.0.1</string><key>SockServiceName</key><string>53535</string><key>SockFamily</key><string>IPv4</string><key>SockType</key><string>dgram</string></dict>
</dict>
```

The daemon gets the sockets with `launch_activate_socket(name, &fds, &count)` (libSystem FFI), converts them to `std::net::TcpListener`, sets them non-blocking, and hands them to tokio.

Because launchd owns the listening sockets, connections made while the daemon restarts queue in the backlog instead of being refused.

This approach is **VERIFY V1**, with fallbacks:
- **(a)** The daemon binds `127.0.0.1:443` itself. Since Mojave, unprivileged binds below 1024 are often allowed. Test it.
- **(b)** Bind `0.0.0.0:443`/`[::]:443` and reject non-loopback peers before TLS. This may trigger an Application Firewall prompt, so record it.
- **(c)** Last resort: setup adds a `pf` anchor that redirects 443 to 8443 on `lo0`. This needs privilege and boot persistence, so ask the human first.

### 8.2 Daemon process

`lokal daemon run` flags (for tests):
- `--base-dir <dir>`
- `--https-port <p>` and `--http-port <p>`: bind directly on 127.0.0.1 and ::1 instead of using launchd sockets
- `--name-backend <auto|mdns-local-only|none>`
- `--foreground`: logs go to stderr too
- `--max-client-connections <n>`, `--max-upstream-connections <n>`, `--pre-request-timeout-ms <ms>`, `--idle-timeout-ms <ms>`: override the connection limits (§9.3)

The daemon:
- Guards against a second instance with an exclusive lock on `daemon.lock`.
- Raises its soft open-file limit (`RLIMIT_NOFILE`) to 10240, or to the hard limit if that's lower, before it starts serving. launchd starts agents at 256. The connection caps are sized from the result (§9.3).
- Logs with `tracing` to `~/Library/Logs/lokal/daemon.log` (daily rotation, keep 3).
- **Startup:** load state, register all names and pre-warm all certs in parallel, then start accepting. Target: under 300ms from launch to serving every configured name.
- **SIGTERM/SIGINT:** stop accepting, finish in-flight requests for up to 1s, deregister names, exit.

### 8.3 Start, stop, restart, status, logs

| Command | Implementation |
|---|---|
| `start` | `launchctl bootstrap gui/<uid> <plist>` if not loaded, otherwise `launchctl kickstart gui/<uid>/dev.lokal.daemon`; then wait for `ping` |
| `stop` | `launchctl bootout gui/<uid>/dev.lokal.daemon`. The agent stays installed and comes back at next login, or with `start` |
| `restart` | `launchctl kickstart -k gui/<uid>/dev.lokal.daemon`; then wait for `ping` |
| `status` | IPC `status`, or `running: false` with `setup_done` from `install.json` |
| `logs` | tail `daemon.log`; `-n` defaults to 50, `-f` follows |

### 8.4 IPC

- Unix socket `<base>/lokal.sock`, mode 0600.
- Newline-delimited JSON, one request per connection.

| Request | Response |
|---|---|
| `{"op":"ping"}` | `{"ok":true,"version":..,"build_id":..,"pid":..}` |
| `{"op":"status"}` | the `status --json` body |
| `{"op":"reload"}` | re-reads state, applies the diff, and responds only when it's done: `{"ok":true,"ms":38,"ready":["api.myproject.local"],"removed":[],"failed":[{"fqdn":..,"error":..}]}` |
| `{"op":"selftest","action":"begin"\|"end"}` | publishes or removes the selftest name |

- The CLI warns if `ping.build_id` differs from its own: `the background service runs a different lokal build — run 'lokal upgrade'`.

## 9. Proxy

### 9.1 TLS

- rustls 0.23 with the `ring` provider, TLS 1.2 and 1.3.
- ALPN advertises `h2` and `http/1.1`. The negotiated protocol picks the server: `h2` gets HTTP/2, anything else (or no ALPN) HTTP/1.1. lokal never sniffs a connection's first bytes to choose, because that wait had no deadline.
- Certificates come from the on-demand resolver (§6.2).
- Session resumption is enabled (tickets plus a stateful cache).
- **Never** send HSTS headers.

### 9.2 Routing and forwarding

Routing:
- Route by the Host header, falling back to the SNI.
  - Host and SNI may legitimately differ under h2 connection coalescing, since all our certs are single-name. If the Host belongs to a different FQDN than the connection's SNI, respond `421 Misdirected Request` so the browser retries on a new connection.
- The routing table is an immutable map behind an `ArcSwap`, swapped atomically on reload. Lookups are lock-free.
- Unknown host: `404` with a tiny HTML page listing that project's URLs, or all projects if the apex is unknown.
- HTTP port 80 (HTTP/1.1 only): `308` redirect to the `https://` URL for known hosts; `404` otherwise.

Upstream connection:
- HTTP/1.1, using the hyper-util client with a connection pool per upstream (keep-alive, idle timeout 90s).
- **`localhost` upstreams:** try `::1` and `127.0.0.1` in parallel (happy-eyeballs style) and remember the winner for each service for 30s. Node 17+ binds `localhost` to `::1` only; Python binds to `127.0.0.1`. Both must work with the default `--host localhost`.
- Connect timeout 2s, then `502` with a page that says: `api.myproject.local → localhost:4000 is not responding. Is the service running?`.
- No read or idle timeout on proxied requests, so long-polling and SSE work.
- If the upstream connection cap is reached (§9.3), a request that needs a new upstream socket gets `503` at once.

Bodies:
- Requests and responses are streamed in both directions, with no buffering and no compression.
- SSE events are flushed immediately.

WebSockets and upgrades:
- `Connection: Upgrade` requests are proxied by upgrading both sides and copying bytes bidirectionally. Required for Vite, Next.js and webpack hot reload.
- RFC 8441 (WebSocket over h2) is not needed; browsers fall back to HTTP/1.1 for WebSockets.

Headers:
- Strip hop-by-hop headers, except for upgrade handling.
- Set `X-Forwarded-Proto: https`, `X-Forwarded-Host: <original host>` and `X-Forwarded-Port: 443`, and append the client IP to `X-Forwarded-For`.
- `host_header = "preserve"` (default) sends the original Host. `host_header = "upstream"` rewrites it to `<host>:<port>`.
- The `service add` long help explains Vite's `server.allowedHosts` and the `--host-header upstream` workaround.

Errors and logs:
- Error pages are tiny inline HTML, or JSON when the request has `Accept: application/json`.
- One access-log line per request at info level: method, host, path, status, duration, upstream.
- Logging must never block the request path; use a non-blocking appender.

### 9.3 Connection limits

Every account on the Mac can connect to the loopback listeners, so a connection that does no proxy work must not hold one of the daemon's file descriptors for long. Without these limits, a few hundred idle sockets used every descriptor, and every lokal URL and the CLI's IPC stopped working until they closed (security audit, 2026-10-23).

- **In flight** means a proxied exchange: from sending the request to the upstream until the upstream response body is done, or for as long as an upgraded connection (WebSocket) stays open. Pages lokal builds itself (redirects, 404, 421, 502, 503, the selftest) never count, so a peer that stops reading one doesn't hold its connection open.
- **Deadlines.** A client connection with nothing in flight is closed 20s after accept (port 80) or after the TLS handshake (443) if it hasn't sent a request, and 60s after its last request (or proxied exchange) otherwise. Proxied requests have no deadline (§9.2). The TLS handshake keeps its own 10s limit, and hyper also closes an HTTP/1.1 keep-alive connection that sends no next request within 30s.
- **Closing.** Before its first request a connection is dropped. After one, it gets a graceful close (GOAWAY, or the end of the current response) for up to 2s, then it is dropped. Neither depends on the peer.
- **Caps.** A quarter of the open-file limit (at least 32, at most 512) is kept free for the listeners, IPC, logs, mDNS and name lookups. Client connections and upstream sockets share the rest equally, each with its own cap: 4864 each at a limit of 10240. Separate caps mean admitting a client never takes the descriptor its upstream connect needs. Name lookups for a `--host` that isn't an IP address run at most 16 at a time.
- **Eviction.** When the client cap is full, a new connection closes the client connection idle longest (nothing in flight; including one still in the TLS handshake) and takes its place. A new connection is refused only when every admitted one has something in flight.
- **Upstream sockets** hold their place while they are open: idle in the pool, or inside an upgraded connection. At the cap, a request that needs a new socket gets `503`.
- **HTTP/2** connections allow 100 concurrent streams and send a keep-alive ping every 20s; a peer that doesn't answer within 10s is dropped.

What remains: a peer that keeps proxied exchanges or WebSockets open to a running upstream can still fill the caps, because lokal can't tell those from long-polling. The caps still keep the daemon's IPC and listeners working then.

## 10. Code structure and future platforms

macOS is the only implementation, but the boundary for other platforms is drawn now so that porting means adding a module, not editing the core.

### 10.1 Platform seam

```rust
// src/platform/mod.rs
//
// PORTING: everything OS-specific lives behind these traits. To add an OS,
// create src/platform/<os>/ implementing them and add a cfg arm in `current()`.
// See SPEC Appendix A for what each trait needs on Linux and Windows.

pub trait TrustStore {
    /// Commands that must run as root to trust the CA. Shown to the user before running.
    fn plan_trust(&self, ca: &CaCert) -> Vec<PrivilegedCommand>;
    fn plan_untrust(&self, ca: &CaCert) -> Vec<PrivilegedCommand>;
    /// Does the OS trust a leaf issued by this CA? (no root)
    fn is_trusted(&self, ca: &CaCert) -> Result<TrustStatus>;
    /// PEM of every lokal CA in the system store (no root), to untrust one whose files are gone.
    fn lokal_cas_pem(&self) -> Result<String>;
    /// Per-user extras such as browser profiles. No root.
    fn apply_user_extras(&self, ca: &CaCert) -> Result<Vec<String>>;
    fn remove_user_extras(&self) -> Result<()>;
}

pub trait NamePublisher: Send {
    fn backend_name(&self) -> &'static str;
    /// Make exactly `fqdns` resolve to loopback. Returns per-name results once settled.
    async fn apply(&mut self, fqdns: &BTreeSet<String>) -> Vec<NameResult>;
    async fn clear(&mut self);
}

pub trait ServiceManager {
    fn install(&self, ctx: &InstallCtx) -> Result<()>;   // no root on macOS
    fn uninstall(&self, ctx: &InstallCtx) -> Result<()>;
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn restart(&self) -> Result<()>;
    fn is_installed(&self) -> bool;
    /// Daemon side: listeners handed over by the service manager (launchd sockets),
    /// or None to make the daemon bind directly.
    fn inherited_listeners(&self) -> Result<Option<Listeners>>;
}

pub trait Desktop {
    fn open_url(&self, url: &str) -> Result<()>;
}

pub fn current() -> Platform { /* cfg(target_os = "macos") => macos::platform(), _ => unsupported::platform() */ }
```

Rules:
- `src/platform/macos/` holds everything macOS-specific: `security`, `launchctl`, the `dns_sd` and `launch_activate_socket` FFI, plist writing, Firefox profiles, and `open`.
- `src/platform/unsupported.rs` compiles on every other OS. Every method returns `Err(Unsupported)` with the message `lokal currently supports macOS only`. The crate must therefore still **compile** on Linux; a CI job checks this (§13).
- **No `cfg(target_os = ...)` and no macOS API outside `src/platform/`.** A test enforces it by scanning `src/**/*.rs` (excluding `src/platform/`) for `target_os` and fails if it finds any.
- Every trait method and every macOS-specific assumption in the core (e.g. "ports come from launchd") carries a `// PORTING(linux): …` or `// PORTING(windows): …` comment, summarising the relevant line of Appendix A.
- Every shell-out goes through one `run(cmd, args)` helper that logs the exact argv at debug level and includes stdout, stderr and the exit status in errors. Never use `sh -c` or string-built commands.

### 10.2 Repository layout

```
Cargo.toml                    workspace (edition 2024, MSRV pinned)
crates/lokal/src/
  main.rs
  cli/                        clap definitions, root help template, output and formatting
  state/                      schema, load/save, locking, validation
  paths.rs                    the only place paths are built
  ca/                         CA and leaf issuance, on-demand resolver, cache
  proxy/                      routing table, forwarding, upgrades, error pages
  daemon/                     startup, reload orchestration, IPC server, selftest
  ipc/                        protocol types and client
  setup/                      onboarding and uninstall orchestration (calls platform traits)
  doctor/
  platform/{mod.rs, macos/, unsupported.rs}
crates/testkit/src/bin/       dev-only, never shipped
  echo-server.rs              HTTP/WS/SSE echo: --bind <ip>, --port, --marker
  check.rs                    probes: resolve | tls | ws | sse | bench  (TLS via rustls-platform-verifier)
  live-test.rs                the live test runner (docs/TESTING.md)
  clean-check.rs              asserts no lokal traces after uninstall
scripts/live-test.sh          thin wrapper: build the testkit, run live-test
docs/PORTING.md               Appendix A, expanded as porting knowledge accumulates
docs/history/PROGRESS.md      the v1 build log: decisions and VERIFY results
```

`echo-server` replies to every request with JSON: the marker, method, path, the headers it received, and the address it bound. `GET /sse` streams 3 events 200ms apart. `/ws` echoes messages. `GET /slow?ms=N` waits before replying.

## 11. Doctor

Each check has an `id`, a `status` (`pass`, `warn`, `fail` or `skip`), a `detail` with a fix hint, and `ms`.

| ID | Checks | On failure |
|---|---|---|
| `setup.done` | `install.json` exists and the plist exists | fail → `lokal setup` |
| `ca.files` | CA files exist; key is 0600 and owned by the user | fail |
| `ca.trusted` | §6.3 check | fail → `lokal setup` (it will ask for the password) |
| `ca.trust_scope` | the CA is trusted for TLS only: the §6.3 scope check doesn't pass (skip when `ca.trusted` fails) | warn → `lokal setup` narrows it (it will ask for the password) |
| `ca.scope` | the CA's name constraints are exactly `local` + `lokal`, with every IP address excluded (§6.1) | warn → `lokal setup` replaces the CA (it will ask for the password) |
| `daemon.running` | IPC `ping` | fail → `lokal start`; include the last 5 log lines |
| `daemon.build` | daemon `build_id` equals CLI `build_id` | warn → `lokal upgrade` |
| `listeners` | daemon reports its listeners; TCP connect to 127.0.0.1:443 and [::1]:443 | fail; if the port is busy, name the process (`lsof -nP -iTCP:443 -sTCP:LISTEN`) |
| `selftest` | the full selftest flow (§4.6 step 4) | fail, naming the layer |
| `dns.<fqdn>` | for each configured FQDN, `getaddrinfo` returns 127.0.0.1 and ::1; report the time | fail if wrong or missing; warn if over 20ms |
| `dns.conflict.<fqdn>` | `/etc/hosts` has no entry for the FQDN pointing elsewhere | warn |
| `tls.<fqdn>` | handshake verified by `rustls-platform-verifier` | fail |
| `upstream.<fqdn>` | TCP connect to the upstream (50ms) | warn: "is your service running?" |
| `firefox` | if installed: whether trust is configured per V5 | warn |
| `resolver.lokal` | `/etc/resolver/lokal` points at `127.0.0.1:53535` and `scutil --dns` shows it active | fail → `lokal setup` |
| `tld.lokal` | a DNS root server (a/b.root-servers.net, RD set because some networks intercept port 53) answers NXDOMAIN for `lokal.` | fail if `.lokal` is delegated (lokal would shadow real sites; move projects to `.local`); skip if offline |

Doctor runs all checks concurrently and finishes in under 1s on a healthy system.

## 12. VERIFY items (confirm on the real Mac before building on them)

Each item has a minimal experiment. Record the exact commands, macOS version and results in `docs/history/PROGRESS.md`.

| ID | Assumption | Experiment (no sudo unless noted) | Fallback |
|---|---|---|---|
| V1 | A **user LaunchAgent** can declare `Sockets` on `127.0.0.1:443` / `::1:443` / `:80`, and `launch_activate_socket` returns the fds | Throwaway agent `dev.lokal.verify.<rand>` running a 30-line test binary that serves "ok"; `curl -k https://127.0.0.1/`; then `bootout` | §8.1 (a) → (b) → (c) |
| V2 | `dns_sd` LocalOnly A+AAAA records are visible to `getaddrinfo`, `dscacheutil -q host -a name`, curl, Safari and Chrome; latency meets §7; **no Local Network privacy prompt** | Test binary registers `v2-<rand>.local` and a nested `api.v2-<rand>.local`; measure lookups; compare the Unique and Shared flags | Appendix B hosts-file backend |
| V3 | A **name-constrained** root is accepted by Safari, Chrome, Firefox, curl, `security verify-cert` and Node `--use-system-ca` | Needs a trusted CA, so it runs right after human gate G1. Start a selftest (IPC `selftest begin`), then hit the selftest name with Chrome headless, `security verify-cert`, curl and `node --use-system-ca`. Live checks L06–L08 re-confirm it later | Remove the constraint (needs a new CA → human gate), plus a doctor warning |
| V4 | `sudo security add-trusted-cert -d ...` from Terminal works with just the sudo password (no extra GUI dialog), or what dialog appears | Observed by the human during G1 | Adjust setup's messaging only |
| V5 | Firefox trusts keychain-trusted roots by default (`security.enterprise_roots.enabled`) | After G1, check in Firefox if installed (headless screenshot or a human glance) | Write the `user.js` pref (§4.6) |
| V6 | `/usr/bin/curl` uses keychain trust | After G1: `curl https://<fqdn>/` with no flags | Document it in the README; `lokal env` sets `CURL_CA_BUNDLE` |
| V7 | `/etc/resolver/lokal` + the built-in DNS server make `*.lokal` resolve for getaddrinfo, curl, Chrome, Node, Python; additions visible at once | After the setup that installs the resolver; also a lookup *before* `service add` | Wildcard answers (§7.2) — measured necessary |
| V8 | `add-trusted-cert -p ssl` trusts the CA for TLS server evaluation only, replaces an entry made without `-p`, and `verify-cert -p basic` tells the two apart without privileges | `crates/testkit/src/bin/verify_trust_policy.rs`: a throwaway CA with lokal's name constraints (its key never written), trusted by the human without `-p`, then with `-p ssl`; leaves for TLS server, TLS client, S/MIME and code signing. **Passed** on macOS 27.0.1 (26A434): with no policy every leaf verifies; after `-p ssl` only the TLS server leaf does, and `dump-trust-settings -d` shows one `Policy OID : SSL` entry. One sudo password and one macOS dialog per command | Untrust, then trust with `-p ssl` |

## 13. Crates and CI

Crates:
- **CLI and config:** `clap` 4 (derive, wrap_help), `serde`, `toml`, `serde_json`, `directories`
- **Errors and logging:** `anyhow`, `thiserror`, `tracing`, `tracing-subscriber`, `tracing-appender`
- **Async and HTTP:** `tokio` (full), `hyper` 1, `hyper-util`, `http-body-util`
- **TLS and certs:** `rustls` 0.23 (`ring`), `tokio-rustls`, `rcgen`, `rustls-platform-verifier`
- **Daemon internals:** `arc-swap`, `notify`

Hand-written FFI for `dns_sd` and `launch_activate_socket`.

Dev-dependencies: `assert_cmd`, `predicates`, `tempfile`, `tokio-tungstenite`, `x509-parser`.

CI (GitHub Actions), at `.github/workflows/ci.yml`:
- `macos-latest`: fmt, clippy (`-D warnings`), and all tests that don't need setup.
- `ubuntu-latest`: `cargo check --workspace` only, which proves the unsupported-platform build compiles. This runs on GitHub, not on the developer's machine.

If the repo has no GitHub remote, write the workflow anyway, skip the "CI is green" criteria, and list "CI not yet run (no remote)" in `docs/history/PROGRESS.md`. Locally, the `target_os` scan test guards the platform seam.

## 14. Performance targets

Measured by the live test (L23) on the developer's Mac.

| Metric | Target |
|---|---|
| `service add` wall time, CLI start to "ready" | < 300ms (p95 of 10) |
| First request right after `service add` returns | succeeds on the first try |
| Name lookup (`getaddrinfo`, A+AAAA) | < 20ms first, < 5ms repeated |
| Proxy overhead on keep-alive requests vs direct upstream | p95 < 2ms |
| New TLS connection (resumed session) | p95 < 10ms |
| Daemon start to serving all names (`restart`) | < 500ms |
| `lokal list` / `lokal status` | < 30ms |
| Daemon idle CPU | < 0.5% averaged over 10s |
| Daemon RSS with 20 services | < 30 MB |

## 15. Milestones and acceptance criteria

A milestone is done only when **all** of its criteria pass, using the checks in `CONTRIBUTING.md` ("Test").

The order follows the developer's flow: **base structure → onboarding (the developer runs `lokal setup` once) → everything else, built and tested live against the installed system.**

### M0: Skeleton and VERIFY
- Workspace and layout per §10.2. Platform traits exist, with macOS stubs (`todo!()`) and `unsupported.rs`.
- The `target_os` scan test exists.
- `docs/PORTING.md` exists, copied from Appendix A.
- **VERIFY V1 and V2** (§12) are done with throwaway experiment binaries, with no sudo. Results are recorded, and the main path or fallback is chosen for listeners and names. If a fallback needs a privileged step, ask the developer before M1, because it changes onboarding.
- CI is green (§13).

### M1: Onboarding slice (ends with human gate G1)
Build the thinnest vertical slice that `lokal setup` needs, so the developer can run it as early as possible:
- `Paths`, minimal state loading (an empty or missing state is fine), and `install.json`.
- **The CA, exactly per §6.1.** It freezes at G1, so a test checks every field with `x509-parser` before the gate.
- Leaf issuance and the on-demand resolver (enough to serve the selftest name).
- The daemon: launchd socket handoff (or the V1 fallback), the name publisher (or the V2 fallback), and the IPC `ping`, `status` and `selftest` ops, serving `200 lokal ok` for the selftest name.
- Commands: `setup` (including `--dry-run`, `--yes`, `LOKAL_FORBID_SUDO`, and the idempotent no-sudo path), `uninstall`, `upgrade`, `start`, `stop`, `restart`, `status` and `logs`. Other commands may still print `not yet implemented (Mx)`.
- The hidden test-only flag `setup --skip-trust` installs everything except the trust step. Its selftest checks the TLS handshake against the CA file instead of macOS trust, and reports `trust: skipped`. `uninstall` only calls sudo when the CA is actually trusted, so after `--skip-trust` it needs none.
- **Before G1, the agent proves the whole slice on the real Mac without sudo:**
  1. `setup --skip-trust`, then `status`, which shows `listener_source: launchd` (or the chosen fallback);
  2. the selftest passes, apart from macOS trust;
  3. `upgrade`, `restart` and `kill -9` recovery all work;
  4. `uninstall` leaves no LaunchAgent behind.
- **Human gate G1** (`docs/history/AGENTS-bootstrap.md` §2): the developer runs `./target/release/lokal setup` once.
- After G1:
  - VERIFY V3–V6 are done;
  - `doctor`'s `ca.trusted` and `selftest` checks are implemented and pass;
  - live checks L01 and L02 pass.

### M2: CLI and state
- The full command tree in §4.1 parses. Commands that aren't implemented yet print `not yet implemented (Mx)` and exit 1.
- Root help matches §4.2: generated from the clap tree, with a test that every leaf command appears.
- Project and service CRUD, `list`, current-project resolution and every rule in §4.3 work, with an integration test per rule using `LOKAL_HOME`.
- `--json` output for `list`, `project list/show` and `service list` matches §4.8.
- 20 parallel `service add` invocations lose no writes.

### M3: Engine
- Proxy, routing, reload and per-service names, per §7–§9.
- Sandbox integration tests run against `lokal daemon run --foreground --https-port <p> --http-port <q> --name-backend none`, with a `LOKAL_HOME` sandbox CA and `check ... --connect 127.0.0.1:<p> --cacert <sandbox ca>`. They pass for:
  - HTTP/1.1 and h2;
  - SSE (each event within 300ms);
  - WebSocket echo;
  - a 50 MB streamed POST;
  - a 502 when the upstream is down;
  - a 404 for an unknown host;
  - the 308 redirect;
  - `X-Forwarded-*` headers;
  - both `host_header` modes;
  - the 421 on Host/SNI mismatch;
  - `localhost` upstreams bound only to `::1` and only to `127.0.0.1`.
- Unknown or missing SNI fails the handshake.
- IPC `reload` returns only once routes are live, names are registered and certs are pre-warmed. Mutating CLI commands print `ready in Nms`. In-flight requests to other services survive a reload.
- An integration test with `--name-backend mdns-local-only` asserts that `getaddrinfo` sees random `lt-<rand>.local` names as soon as `reload` returns, and that they're gone as soon as removal returns.
- `scripts/live-test.sh` passes L01–L22 and L24 against the installed service (after `lokal upgrade`).

### M4: Polish and speed
- `doctor` (all checks in §11, < 1s), `env`, `ca path`, `open`, the Firefox handling, and the before-setup nudges and welcome.
- All performance targets in §14 pass (L23).
- README: setup, quick start, `lokal env`, the Vite note, uninstall, and the trust model in plain words.
- **Human gate G2** (final clean room, `docs/history/AGENTS-bootstrap.md` §2) passes.

### Releases (built on request after v0.1)
- A `vX.Y.Z` tag runs CI, builds ad-hoc-signed `aarch64-apple-darwin` and `x86_64-apple-darwin` tarballs with
  SHA-256 checksums, and creates a draft GitHub release. Publishing the draft updates the Homebrew tap
  (`brew install mehmetkose/tap/lokal`) and installs from it on a fresh runner. Details:
  `docs/RELEASING.md`, `docs/PACKAGING.md`.

### M5: Backlog (only on request)
- a `lokal.toml` project file with `lokal up`: every service has a `run` command (plan: `docs/ROADMAP.md` §1)
- error trips across services, pushed to the coding agent (plan: `docs/ROADMAP.md` §4)
- `lokal share`: public URLs through Cloudflare Tunnel, using your own Cloudflare account (plan: `docs/ROADMAP.md` §5)
- LAN mode (publish on real interfaces for phone testing)
- upstream HTTPS
- `service enable/disable`
- shell completions
- Linux support (Appendix A)

## 16. Security requirements (non-negotiable)

1. The CA key is mode 0600, owned by the user, and is never logged, copied or transmitted.
2. The CA has name constraints, unless V3 forces their removal; in that case doctor warns.
3. Certificates are issued only for configured FQDNs and the active selftest name.
4. Listeners are loopback-only (unless fallback §8.1 (b), which must reject non-loopback peers before TLS).
5. The IPC socket is mode 0600.
6. Privileged actions are limited to the commands listed by `TrustStore::plan_*` and `SystemResolver::plan_*` (installing/removing `/etc/resolver/lokal`). They are printed before they run, and run only in `setup` and `uninstall`. Nothing runs as root at runtime.
7. No `sh -c` and no string-interpolated commands.
8. `uninstall` reverts everything recorded in `install.json`.
9. `.lokal` is not a reserved TLD. The CA's name constraints limit it to `.lokal` and `.local` (no IP addresses), and doctor's `tld.lokal` check fails loudly if `.lokal` is ever delegated in the public DNS.
10. The CA is trusted for the SSL policy only (`add-trusted-cert -p ssl`, §6.3), never for S/MIME, code signing or any other use. Doctor's `ca.trust_scope` warns otherwise.

---

## Appendix A: Porting notes (not built in v1)

This research is kept so future ports don't start from zero. Copy it into `docs/PORTING.md` at M0.

**Linux:**
- **TrustStore.**
  - Ubuntu/Debian: copy to `/usr/local/share/ca-certificates/lokal-ca.crt` and run `update-ca-certificates`.
  - Arch/Fedora: `trust anchor --store` and `update-ca-trust`.
  - Detect by command presence, not `os-release`.
  - Chrome and Firefox use NSS databases (`~/.pki/nssdb`, and Firefox profiles including the snap path `~/snap/firefox/common/.mozilla/firefox/`). Update them via `certutil` **as the user, never as root**.
- **ServiceManager.** systemd socket activation (`lokal.socket` with `ListenStream=127.0.0.1:443` etc., read with `sd_listen_fds`) is the direct equivalent of launchd Sockets. Alternatively, a system unit with `AmbientCapabilities=CAP_NET_BIND_SERVICE`.
- **NamePublisher.**
  - Avahi over D-Bus is the mDNS equivalent, but it publishes to the LAN, and multi-label `.local` names need nss-mdns configuration.
  - The more reliable option is a root oneshot unit triggered by a `.path` unit, which syncs a validated, user-written desired-hosts file into a managed `/etc/hosts` block.
  - For `.lokal`: systemd-resolved split DNS via a dummy link (`resolvectl dns lokal0 127.0.0.1:53535`, `resolvectl domain lokal0 '~lokal'`).

**Windows:**
- **TrustStore:** `certutil -addstore Root` or `CertAddCertificateContextToStore`.
- **NamePublisher:** the hosts file via an elevated helper, or NRPT rules for a custom TLD.
- **ServiceManager:** a Windows service, or a per-user scheduled task. Port 443 needs no special privilege for loopback binds.

## Appendix B: `hosts-file` fallback backend (only if V2 fails)

- **At setup**, add privileged steps:
  - install `/Library/LaunchDaemons/dev.lokal.hosts.plist` (root), with `WatchPaths` on `/Library/Application Support/lokal/hosts.desired`, running `/Library/Application Support/lokal/bin/lokal-hosts-sync`. This is a root-owned copy of the binary, and it must not be user-writable.
  - create the `hosts.desired` file owned by the user.
- **The daemon** writes the desired file atomically. Each line is `127.0.0.1 <fqdn>` or `::1 <fqdn>`.
- **The sync job** treats its input as untrusted:
  - the IP must be exactly `127.0.0.1` or `::1`;
  - the hostname must pass §4.3 and end in an allowed TLD;
  - the file is at most 10,000 lines.
  
  It rejects the whole file on any violation. Otherwise it replaces only the block between `# BEGIN lokal (managed)` and `# END lokal`, writes via temp file and rename, preserves mode and owner, and runs `dscacheutil -flushcache; killall -HUP mDNSResponder`.
- **`reload` readiness** for this backend means `getaddrinfo` returns the new set (poll every 10ms, up to 1s).
- **`upgrade`** cannot replace the root-owned sync binary. It needs `setup` again, which costs one sudo.
