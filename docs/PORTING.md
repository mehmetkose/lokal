# Porting lokal

lokal v1 runs on macOS only, but everything OS-specific sits behind the traits in
`crates/lokal/src/platform/mod.rs` (`TrustStore`, `NamePublisher`, `ServiceManager`,
`Desktop`, `System`, `SystemResolver`). To add an OS, create `src/platform/<os>/` implementing them and
add a `cfg` arm in `platform::current()` and `platform::name_publisher()`.
`src/platform/unsupported.rs` is what other OSes compile today: every operation fails
with "lokal currently supports macOS only", while the state commands and the sandbox
daemon (`lokal daemon run --https-port … --name-backend none`) still work.

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
| Paths | `~/Library/Application Support/lokal`, `~/Library/Logs/lokal`, `~/Library/LaunchAgents` (`paths.rs`) | XDG dirs / `%APPDATA%` |

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
