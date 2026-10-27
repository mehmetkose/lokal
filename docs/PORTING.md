# Porting lokal

lokal ships for macOS only, but everything OS-specific sits behind the traits in
`crates/lokal/src/platform/mod.rs` (`TrustStore`, `NamePublisher`, `ServiceManager`,
`Desktop`, `System`, `SystemResolver`). To add an OS, create `src/platform/<os>/` implementing them and
add a `cfg` arm in `platform::current()`, `platform::name_publisher()` and `platform::user_dirs()` (the log
dir and the service definition; `paths.rs` builds every path from them).
`src/platform/unsupported.rs` is what other OSes compile today: every operation fails
with "lokal currently supports macOS only", while the state commands and the sandbox
daemon (`lokal daemon run --https-port … --name-backend none`) still work.

## Linux: what's done

`src/platform/linux/` has the pieces that need no privileges, and CI runs clippy and the whole test suite on
Linux, x86_64 and arm64 (`ubuntu-latest` and `ubuntu-24.04-arm`):

- **`System`:** `port_owners` runs `ss -H -t -a -n -p 'sport = :<port>'` and keeps the processes of the
  unconnected sockets (listening, or bound with no peer), with the process group from field 5 of
  `/proc/<pid>/stat`; `tcp_listen_ports` walks every process's ppid from `/proc/<pid>/stat` with `process_tree`
  and keeps the listeners in `ss -H -t -l -n -p` that belong to that tree. ss comes from iproute2 and runs from an
  absolute path. Only the user's own processes are named, as with lsof on macOS. The OS is `PRETTY_NAME` from
  `/etc/os-release`. VERIFY V9, V10 and V11 have Linux results in SPEC §12, from `verify_linux_tree`,
  `verify_process_groups` and `verify_ss_port_owners`.
- **`Desktop`:** `/usr/bin/xdg-open`.
- **Paths:** the base dir is `$XDG_DATA_HOME/lokal` (`~/.local/share/lokal`), the log dir and `lokal up`'s folders
  `$XDG_STATE_HOME/lokal` (`~/.local/state/lokal`).
- **Not yet:** trust, the background service and name resolution, so there's no service definition and lokal is
  never set up: `lokal setup` fails with "lokal currently supports macOS only", and the commands that need the
  background service say lokal isn't set up. They come in later changes, along with Linux release builds.

The parsers (`/proc/<pid>/stat`, ss lines, os-release) are in `src/platform/procfs.rs`, which compiles on every OS,
so `cargo test` on a Mac covers them too. On a Mac, lint the Linux code with
`cargo clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`.

[`ARCHITECTURE.md`](ARCHITECTURE.md#the-platform-seam) summarises what each trait does. If you start a port, open
an issue first so the work can be coordinated.

Rules (enforced by `crates/lokal/tests/seam.rs`):
- no `cfg(target_os)` and no OS command or API outside `src/platform/`;
- every trait method has a `PORTING(linux|windows)` note.

## What macOS does today, and the equivalent elsewhere

| Concern | macOS (v1) | Notes for other OSes |
|---|---|---|
| Trust | `security add-trusted-cert -d -r trustRoot -p ssl -k /Library/Keychains/System.keychain` (sudo, setup only); checked with `security verify-cert -c leaf -c root -p ssl -L` (root passed as a chain cert, **not** `-r`, which would make it an anchor) | see below |
| Ports 443/80 | LaunchAgent `Sockets` (launchd binds, hands fds over via `launch_activate_socket`); `ThrottleInterval` 1 for instant respawn | systemd socket activation; Windows needs no privilege for loopback 443 |
| Names | mDNSResponder `DNSServiceRegisterRecord` A+AAAA on `kDNSServiceInterfaceIndexLocalOnly`, explicit `DNSServiceRemoveRecord` on removal | see below |
| `.lokal` | Built-in DNS server (portable, `src/dns/`) on a launchd UDP socket 127.0.0.1:53535; `SystemResolver` installs `/etc/resolver/lokal` (sudo, setup). Wildcard loopback answers, TTL 0 (mDNSResponder caches any failure ~75s) | Linux: systemd-resolved split DNS (`resolvectl dns <link> 127.0.0.1:53535`, `resolvectl domain <link> '~lokal'`). Windows: NRPT rule for `.lokal` (needs port 53) |
| Paths | `~/Library/Application Support/lokal`, `~/Library/Logs/lokal`, `~/Library/LaunchAgents` (`paths.rs`, `platform::user_dirs`) | Linux (done): `~/.local/share/lokal`, `~/.local/state/lokal`; the systemd user unit comes with the service. Windows: `%APPDATA%` |
| Port owners and listeners | `lsof -F pgcnT`, `ps -A -o pid=,ppid=` (VERIFY V9, V11) | Linux (done): `ss -H -t -n -p` and `/proc/<pid>/stat`. Windows: `GetExtendedTcpTable`, a process snapshot |

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
