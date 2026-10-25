# Architecture

A short tour of how lokal works and where the code lives. [`SPEC.md`](SPEC.md) has the full detail, and this
document points to its sections.

## The idea

lokal has two phases:

1. **Setup, once per Mac.** This is the only time anything privileged happens. Each one-time step removes a
   runtime privilege for good:

   | Problem | One-time step in setup | Result at runtime |
   |---|---|---|
   | Browsers must trust the certificates | Create a CA as the user, and trust it in the System keychain (sudo) | Leaf certificates are issued in memory. The keychain is never touched again |
   | Ports 443 and 80 are privileged | Install a LaunchAgent whose plist declares the sockets. launchd binds them | The daemon runs as the user and inherits the sockets |
   | `*.lokal` must resolve | Install `/etc/resolver/lokal` (sudo), pointing at `127.0.0.1:53535` | The daemon's DNS server answers on a launchd-owned UDP socket |
   | `*.local` must resolve | Nothing | The daemon registers local-only records with mDNSResponder, unprivileged |

2. **Daily use, fast and quiet.** `project` and `service` commands edit a state file and tell the daemon to
   reload. They return only when the URL works.

## Components

One binary has two modes: the **CLI** (`lokal <command>`) and the **daemon** (`lokal daemon run`, hidden from
help), which the LaunchAgent runs.

```text
 browser / curl / node
   │
   ├─ 1. resolve api.shop.lokal ──▶ mDNSResponder (the macOS resolver)
   │                                  ├─ *.lokal: /etc/resolver/lokal ──▶ lokal's DNS server (UDP 127.0.0.1:53535)
   │                                  └─ *.local: local-only A/AAAA records that lokal registered
   │
   └─ 2. HTTPS to 127.0.0.1:443 ──▶ socket held by launchd ──▶ lokal: TLS with a leaf for the SNI ──▶ proxy ──▶ localhost:4000

 lokal CLI ──▶ state.toml (locked, atomic write) ──▶ IPC "reload" on lokal.sock ──▶ daemon updates routes, certificates, names
                                                     (and replies once every changed name is ready)
```

- **Listeners.** launchd binds TCP 443 and 80 on `127.0.0.1` and `::1`, and UDP 53535 on `127.0.0.1`. The daemon
  receives the sockets with `launch_activate_socket`. Connections that arrive while it restarts wait in the
  backlog. In tests the daemon binds high ports itself (`--https-port`, `--http-port`, `--dns-port`).
- **Certificate issuer.** A rustls `ResolvesServerCert` issues a CA-signed leaf for each configured name,
  pre-warms it on reload, and caches it in memory. Unknown names get no certificate, so the handshake fails.
- **Proxy.** Routes by `Host`, falling back to SNI, through a lock-free table (`ArcSwap`) that is swapped on reload.
  It streams HTTP/1.1 and h2 to upstreams over pooled HTTP/1.1 connections, upgrades WebSockets, and returns
  small error pages (404, 421, 502). Port 80 redirects to HTTPS.
- **Names.** `.lokal`: a small authoritative DNS server that answers every name in the zone with loopback and
  TTL 0 (see [why](SPEC.md#72-lokal-built-in-dns-server)). `.local`: `DNSServiceRegisterRecord` on the
  local-only interface, through hand-written `dns_sd` FFI driven by one tokio task.
- **IPC.** Newline-delimited JSON over `<base>/lokal.sock` (mode 0600), one request per connection: `ping`,
  `status`, `reload`, `selftest`.
- **State watcher.** The daemon also watches `state.toml`, so hand edits apply without a CLI command.

## The fast path

Every mutating command (`project create/remove`, `service add/update/remove`, and `up`, `down` and `run` when
they change routes):

1. takes an exclusive lock on `state.toml.lock`, then reads, modifies and atomically writes `state.toml`
   (temp file, fsync, rename);
2. sends IPC `reload`. The daemon diffs the new state against the old one. It installs routes, issues
   certificates and publishes names, and responds only when every affected name is ready. For `.local` that
   means the mDNS callback has fired for both records;
3. prints the result with the measured time (`ready in 8ms`).

That's why the live test can request a URL right after the command returns, with no retry
([`TESTING.md`](TESTING.md)). Only the CLI writes `state.toml`. The daemon only reads it.

## Files

| What | Path |
|---|---|
| State | `~/Library/Application Support/lokal/state.toml` |
| CA | `…/lokal/ca/root.pem`, `root-key.pem` (0600), `bundle.pem` (system roots + lokal, for `lokal env`) |
| Daemon binary | `…/lokal/bin/lokal`, a copy managed by `setup` and `upgrade` |
| IPC socket, daemon lock | `…/lokal/lokal.sock`, `…/lokal/daemon.lock` |
| Install record | `…/lokal/install.json`, which `uninstall` follows |
| Approvals of `lokal.toml` files | `…/lokal/trusted/<SHA-256 of the file's real path>` (0600, in a 0700 folder), holding the SHA-256 of the approved bytes |
| Logs | `~/Library/Logs/lokal/daemon.log`, rotated daily, 3 kept |
| LaunchAgent | `~/Library/LaunchAgents/dev.lokal.daemon.plist` |

`LOKAL_HOME=<dir>` moves all of these into `<dir>` (logs into `<dir>/logs`). The tests use it as a sandbox.
Every path is built in `paths.rs`.

## Code map

`crates/lokal/src/`:

| Module | Responsibility |
|---|---|
| `main.rs`, `cli/` | clap definitions (`args.rs`), the generated root help (`help.rs`), output and colour (`output.rs`), project and service commands (`projects.rs`), `lokal run` (`run.rs`: route, start and supervise a dev server), `lokal up` and `lokal down` (`up.rs`: list a repo's `lokal.toml` commands for a person to approve, then route every service in it, as pure planning functions and a thin command layer; hidden while being built), `status`, `logs`, `env`, `open` (`system.rs`) |
| `project_file.rs` | a repo's `lokal.toml`: parsing it, checking every rule of SPEC §4.11, and errors and warnings that name `<path>:<line>:<col>`. It resolves the file to its real path, which the approval's key, the `cwd` base and the derived project name all come from, and takes the digest of the bytes it parsed, for the trust check |
| `approval.rs` | a person's approval of a `lokal.toml` (SPEC §16.11): the record in `<base>/trusted/`, read before `lokal up` changes anything and written only after a `y` on a terminal. Not called `trust`, which is the CA's `TrustStore` |
| `state/` | the `state.toml` schema, locked atomic writes, and the validation rules of SPEC §4.3 (`validate.rs`) |
| `paths.rs` | every filesystem location, and `LOKAL_HOME` |
| `ca/` | CA creation and checks (`mod.rs`), the on-demand rustls certificate resolver (`resolver.rs`) |
| `proxy/` | TLS accept loops (`mod.rs`), connection deadlines, caps and eviction (`limits.rs`), the routing table (`routes.rs`), forwarding, upgrades and 421 (`forward.rs`), the `localhost` happy-eyeballs connector (`connector.rs`), error pages (`pages.rs`) |
| `dns/` | the `.lokal` DNS server (`mod.rs`) and the DNS wire format (`wire.rs`) |
| `daemon/` | startup, reload orchestration, selftest, shutdown (`mod.rs`), the IPC server (`server.rs`), logging (`logging.rs`) |
| `ipc/` | protocol types and the client |
| `setup/` | `setup`, `uninstall`, `upgrade`, start and stop (`mod.rs`), and `install.json` (`record.rs`) |
| `doctor/` | every check from SPEC §11, run concurrently |
| `net.rs` | client-side probes: `getaddrinfo`, TCP reachability, verified HTTPS GETs; picking a port free on both loopbacks |
| `runner.rs` | what a started dev server gets: `{port}` substitution and its environment (`PORT`, `LOKAL_URL_<SUB>`, …) |
| `process.rs` | the only place lokal runs other programs: an argument vector, never `sh -c`. That includes the user's own command for `lokal run`, started with the terminal attached |
| `platform/` | the OS seam (below) |
| `error.rs`, `util.rs`, `version.rs` | user-facing errors and exit codes, small helpers, the build id |

`crates/testkit/`: dev-only tools that are never shipped. `echo-server` is the upstream used in tests, `check`
holds the probes, `live-test` and `clean-check` are the live test runner and the uninstall check, and
`verify_sockets` and `verify_mdns` are the macOS experiments.

## The platform seam

Everything OS-specific sits behind traits in [`platform/mod.rs`](../crates/lokal/src/platform/mod.rs):

| Trait | What it does | macOS implementation |
|---|---|---|
| `TrustStore` | plan trust and untrust commands, check trust, find lokal CAs in the store, browser extras, system roots | `macos/trust.rs` (`security`), `macos/firefox.rs` |
| `NamePublisher` | make a set of names resolve to loopback, and report when they're ready | `macos/mdns.rs` (`dns_sd`), plus the portable `.lokal` DNS publisher |
| `ServiceManager` | install, start, stop and restart the daemon, and hand it its listeners | `macos/launchd.rs` (`launchctl`, `launch_activate_socket`) |
| `SystemResolver` | send `.lokal` lookups to the DNS server | `macos/resolver.rs` (`/etc/resolver/lokal`) |
| `Desktop` | open a URL | `/usr/bin/open` |
| `System` | root check, user and host names, run a privileged command, port owners, the listening ports of a process and its descendants, `/etc/hosts`, OS version | `macos/mod.rs` (`lsof`, `ps`) |

`platform/mod.rs` also holds the few unix process calls `lokal run` needs (catching and relaying signals,
the terminal's foreground process group, exit codes from signals), each with a `PORTING(windows)` note, and
`process_tree`, the OS-neutral walk from a pid to its descendants.

Other OSes compile `platform/unsupported.rs`, where every operation fails with "lokal currently supports macOS
only". `tests/seam.rs` keeps `cfg(target_os)` out of the rest of the code. [`PORTING.md`](PORTING.md) collects
what Linux and Windows would need.

## Design decisions worth knowing

- **Wildcard `.lokal` answers.** mDNSResponder caches *any* failed lookup for about 75 seconds. If lokal answered
  NXDOMAIN for unknown names, a URL you opened before `service add` would stay broken for a minute. So every
  `.lokal` name resolves, and "not configured" is enforced at the TLS layer instead.
- **Explicit mDNS removal.** Closing the `dns_sd` connection removes records asynchronously, so lookups could
  briefly still succeed. The daemon removes each record explicitly before it reports a removal as done.
- **`ThrottleInterval` 1 in the plist.** launchd's default 10-second throttle would delay restarting the daemon
  after a crash. With 1, a killed daemon is back in milliseconds.
- **Trust check with `-c`, not `-r`.** `security verify-cert -r <root>` makes the root an anchor and passes even
  when the keychain doesn't trust it. lokal passes the root as a chain certificate instead.

The evidence for each is in the Decisions section of [`history/PROGRESS.md`](history/PROGRESS.md).
