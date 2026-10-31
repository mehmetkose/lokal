# Security model

lokal asks for a lot of trust: it adds a certificate authority to your Mac's System keychain and runs a
background service that terminates TLS for you. This document covers what it changes, what it protects, and
what risks remain. To report a vulnerability, see [`SECURITY.md`](../SECURITY.md).

The normative rules are in [`SPEC.md` §16](SPEC.md#16-security-requirements-non-negotiable). This document
explains them. Most of it applies to the Linux preview too; what differs there is under [On Linux](#on-linux).

## What setup changes on your Mac

| What | Where | Owner | Undone by |
|---|---|---|---|
| Root CA certificate | `~/Library/Application Support/lokal/ca/root.pem` (0644) | you | `uninstall --purge` |
| Root CA private key | `~/Library/Application Support/lokal/ca/root-key.pem` (0600) | you | `uninstall --purge` |
| Trust setting for the CA | `/Library/Keychains/System.keychain` | root (sudo) | `uninstall` |
| Resolver file for `.lokal` | `/etc/resolver/lokal` (`nameserver 127.0.0.1`, `port 53535`) | root (sudo) | `uninstall` |
| LaunchAgent | `~/Library/LaunchAgents/dev.lokal.daemon.plist` | you | `uninstall` |
| Copy of the lokal binary that the agent runs | `~/Library/Application Support/lokal/bin/lokal` | you | `uninstall` |
| Projects and services | `~/Library/Application Support/lokal/state.toml` | you | `uninstall --purge` |
| Certificate bundle for `lokal env` | `~/Library/Application Support/lokal/ca/bundle.pem` | you | `uninstall` |
| Install record (what setup did) | `~/Library/Application Support/lokal/install.json` | you | `uninstall` |
| Firefox preference (only if Firefox is installed) | `user.js` in each Firefox profile | you | `uninstall` |
| Logs | `~/Library/Logs/lokal/` | you | `uninstall --purge` |
| Approvals of `lokal.toml` files (`lokal up`) | `~/Library/Application Support/lokal/trusted/` (0700; one 0600 file per approved file) | you | `uninstall --purge` |
| `lokal up`'s lock per project | `~/Library/Logs/lokal/up/<project>/up.lock` (0600, in a 0700 folder) | you | `uninstall --purge` |
| `lokal up`'s log per started service | `~/Library/Logs/lokal/up/<project>/<sub>.log` and `<sub>.log.1` (0600, in a 0700 folder) | you | `uninstall --purge` |

The data directory is created with mode 0700. lokal doesn't touch `/etc/hosts`, `pf`, the Application Firewall,
other launchd jobs or any other keychain item.

## Privileged commands

lokal runs a fixed set of commands as root, and only in `setup` and `uninstall`. It prints each one before it runs
it, and `lokal setup --dry-run` shows them without running anything. All of them are built in
`TrustStore::plan_*` and `SystemResolver::plan_*` (`crates/lokal/src/platform/macos/`) and run through `sudo`
with an argument vector. lokal never builds a command line from strings, and runs a shell (`sh -c`) in one place
only, below. Two commands start programs you name, as you and never as root:

- `lokal run` starts the command after `--`, with the arguments you typed, where `{port}` becomes the port. No
  shell is involved.
- `lokal up` starts each service's `run` from a repo's `lokal.toml` as `/bin/sh -c '<run>'`, the one shell lokal
  runs (SPEC §16.7), with `{port}` replaced by a number and nothing else added. A `lokal.toml` may be someone
  else's, so `up` first lists every command in it, with its `cwd` and `env`, and asks `run these commands?
  [y/N]` (SPEC §16.11). Only `y` on a terminal records an approval, in `<base>/trusted/`: the SHA-256 of the
  file's exact bytes, keyed by its real path. Any change to the file asks again. With no terminal it changes
  nothing and exits 1, and its message never suggests `--yes`, the flag for agents and CI that runs without
  asking and remembers nothing. Approving a file means trusting the repo: `npm start` runs the repo's own
  scripts, as with direnv's `allow`.

`setup`, on a fresh Mac:

```text
sudo /usr/bin/security add-trusted-cert -d -r trustRoot -p ssl -k /Library/Keychains/System.keychain <base>/ca/root.pem
sudo /bin/mkdir -p /etc/resolver
sudo /usr/bin/install -m 0644 <base>/resolver/lokal /etc/resolver/lokal
```

`uninstall`, and `setup` when it replaces an outdated CA:

```text
sudo /usr/bin/security remove-trusted-cert -d <old root.pem>
sudo /usr/bin/security delete-certificate -Z <SHA-1 of the old CA> /Library/Keychains/System.keychain
sudo /bin/rm -f /etc/resolver/lokal        # uninstall only
```

Setup checks that sudo can run (a terminal is attached, and `LOKAL_FORBID_SUDO` isn't set) **before** it changes
anything. A setup that can't finish therefore never leaves a half-replaced CA behind. Setup and uninstall refuse
to run as root.

After setup, nothing runs as root. The background service is a per-user LaunchAgent. launchd binds ports 443 and
80 and hands the sockets to the service, so the service itself never needs privileges.

## The certificate authority

lokal's root CA is created on your Mac, by you, during setup:

| Field | Value |
|---|---|
| Key | ECDSA P-256, generated locally |
| Subject | `CN=lokal development CA (<user>@<host>)`, `O=lokal` |
| Validity | 10 years |
| Basic constraints | critical, `CA:TRUE`, `pathLen:0`, so it can't create intermediate CAs |
| Key usage | critical, `keyCertSign`, `cRLSign` |
| **Name constraints** | critical. **Permitted DNS names:** `lokal`, `local`. **Excluded IP ranges:** `0.0.0.0/0`, `::/0` |

The name constraints are the most important control. A conforming certificate verifier rejects any certificate
this CA signs for a name outside `.lokal` and `.local`, or for any IP address. This was tested with macOS's own
verifier (`security verify-cert`), in Chrome, and with OpenSSL (`openssl verify`), the library that Node and Python
use. Firefox's NSS library supports name constraints too. The evidence is in
[`history/PROGRESS.md`](history/PROGRESS.md) (iterations #13 and #14).

IP addresses are excluded explicitly because DNS-name constraints alone don't constrain them (RFC 5280
§4.2.1.10 constrains each name form separately). Without the exclusion, macOS and Chrome accepted a
`127.0.0.1` leaf signed by the CA.

The trust setting is limited to HTTPS, too. Setup trusts the CA with `-p ssl`, so macOS accepts it only when it
checks a server's TLS certificate. Name constraints cover only DNS names and IP addresses, so without that limit
a certificate the CA signs would also pass for email (S/MIME, for any address), code signing and TLS client
authentication. This was measured with `crates/testkit/src/bin/verify_trust_policy.rs` (SPEC §12 V8).
`lokal doctor` warns if the CA is trusted for more than HTTPS (`ca.trust_scope`), and `lokal setup` narrows it.

Leaf certificates are issued in memory by the background service, one per configured name. Each has exactly one
DNS name, `serverAuth` usage, and a 90-day validity. They are never written to disk or to the keychain. The
service presents a certificate only for names you configured (plus the setup self-test name, while the
self-test runs). A TLS handshake for any other name, or with no SNI, fails.

The CA's fields are pinned by a test (`ca::tests::ca_fields_match_spec`). Any change to them means users have to
re-trust a new CA, so it needs a deliberate decision (see [`CONTRIBUTING.md`](../CONTRIBUTING.md#rules-for-code-changes)).

## If the CA's private key leaks

The key is a file that only your user account (and root) can read. Any program running as you can read it, just as
it can read your SSH keys or browser cookies. lokal never copies, logs or transmits it.

If someone gets the key, they **can**:

- sign certificates for any `.lokal` name, or any `.local` name, that every account on this Mac would trust.
  That includes `.local` devices on your network, such as a NAS or printer advertised over Bonjour, so an
  attacker who can also intercept your traffic could impersonate them over HTTPS on this Mac.

They **can't**:

- impersonate any public website, or any name under another TLD (a company's `.internal` or `.test` hosts, for
  example);
- impersonate an IP address (`https://192.168.1.1`);
- sign email (S/MIME), code or TLS client certificates that this Mac accepts, because the CA is trusted for
  HTTPS only (on Linux, only the browsers have that limit: see [Trust and browsers on
  Linux](#trust-and-browsers-on-linux));
- create an intermediate CA that escapes these limits (`pathLen:0`, and name constraints apply to the whole
  chain);
- affect other computers, because the CA is trusted only on this Mac.

These guarantees rest on the client enforcing name constraints. The verifiers tested above do. Very old or
unusual TLS stacks might not.

If you think the key leaked, run `lokal uninstall --purge` and then `lokal setup`. This untrusts and deletes the
old CA, and creates a new one.

## Network exposure

| Listener | Address | Reachable from |
|---|---|---|
| HTTPS proxy | `127.0.0.1:443`, `[::1]:443` (TCP) | this Mac only |
| HTTP redirect to HTTPS | `127.0.0.1:80`, `[::1]:80` (TCP) | this Mac only |
| `.lokal` DNS server | `127.0.0.1:53535` (UDP) | this Mac only |
| IPC | `<base>/lokal.sock`, a Unix socket with mode 0600 | your user account |

- `.local` names are registered with mDNSResponder on the **local-only** interface
  (`kDNSServiceInterfaceIndexLocalOnly`), so they are never announced on your network. On Linux, lokal
  publishes no `.local` names at all.
- The DNS server answers only names under `lokal`: `A 127.0.0.1` and `AAAA ::1`, with TTL 0. It refuses
  everything else, so it can't be used to resolve or redirect other names.
- The proxy never sends `Strict-Transport-Security` headers.
- Any account on this Mac can connect to the loopback listeners. A connection that isn't proxying a request
  is closed after a deadline (20s before its first request, 60s idle), and client and upstream connections are
  capped below the daemon's open-file limit, with the idle longest closed first when the cap is full. So idle
  connections can't use up lokal's file descriptors and block it for everyone.
- Upstreams default to `localhost`. lokal warns when you point a service at a host that isn't loopback, and
  refuses loopback ports 443 and 80, because they would loop back into lokal itself.
- Outbound traffic: each `lokal doctor` run sends one DNS query to a public root server (`a.root-servers.net`
  or `b.root-servers.net`) to check that `.lokal` isn't a real TLD. lokal sends nothing else off your Mac. It has
  no telemetry and no update checks.

## The `.lokal` TLD

`.local` is reserved for multicast DNS (RFC 6762). `.lokal` isn't reserved. It isn't delegated in the public DNS
today, but in theory it could become a real top-level domain one day. If that happened, lokal would hide real
`.lokal` websites on your Mac, because `/etc/resolver/lokal` sends those lookups to lokal. On Linux, lokal's DNS
delegate for systemd-resolved does the same.

`lokal doctor`'s `tld.lokal` check asks a DNS root server whether `.lokal` exists, and **fails** if it does. If
that check ever fails, move your projects to `.local` (`lokal project create <name> --domain <name>.local`).

## Firefox

Firefox keeps its own certificate store. If Firefox is installed, setup adds
`user_pref("security.enterprise_roots.enabled", true);` to `user.js` in each Firefox profile, tagged
`// added by lokal`, and uninstall removes exactly that line.

That preference makes Firefox trust **every** root CA that a user or administrator added to the macOS keychain,
not just lokal's. If you don't want that, delete the line.
lokal then won't work in Firefox, but it keeps working everywhere else.

On Linux, lokal sets no Firefox preference. It adds only its own CA to Firefox's certificate database
([Trust and browsers on Linux](#trust-and-browsers-on-linux)).

## Uninstalling

`lokal uninstall` follows `install.json`. It removes the LaunchAgent and its binary, untrusts and deletes every
lokal CA from the System keychain (including retired ones), removes `/etc/resolver/lokal` and removes the
Firefox line. It succeeds, with warnings, if some pieces are already gone.

It picks CAs by certificate, so deleting the key file first doesn't hide a trusted CA from it. If the CA files
are gone altogether, it finds the CA that `install.json` records in the keychain, by the SHA-1 it computes
itself, and untrusts it. `lokal setup` does the same before it creates a new CA: it never writes over a
certificate that may still be trusted.

A plain uninstall keeps `state.toml` and the CA files, so `lokal setup` can restore everything without
creating a new CA. Once the CA is no longer trusted, the key on disk can't make anything trusted. That includes
Node, Python and curl: `lokal env` and `lokal ca path` print nothing once setup is gone, so an
`eval "$(lokal env)"` line left in a shell profile no longer exports the CA. Uninstall also deletes `bundle.pem`
and reminds you to remove that line. If you're done with lokal, `--purge` deletes the data directory, the key
included.

## On Linux

lokal on Linux is a preview (since 0.5.0; [`PORTING.md`](PORTING.md)). The CA, its name constraints, the leaf
certificates, the listeners, the IPC socket and the `lokal.toml` trust check are the same code as on macOS. Setup
reaches the same result with different system pieces (`crates/lokal/src/platform/linux/`): the system certificate
store instead of the keychain, a systemd-resolved DNS delegate instead of `/etc/resolver/lokal`, and systemd user
units instead of a LaunchAgent. Only `.lokal` names work.

### What setup changes on Linux

`<base>` is `~/.local/share/lokal` (`$XDG_DATA_HOME/lokal`). `state.toml`, the CA files, `bundle.pem`,
`install.json` and `trusted/` are under it, as on macOS. The rest differs:

| What | Where | Owner | Undone by |
|---|---|---|---|
| The CA in the system store | `lokal-<sha1>.crt` in the anchors folder: `/usr/local/share/ca-certificates/` (Debian, Ubuntu), `/etc/pki/ca-trust/source/anchors/` (Fedora) or `/etc/ca-certificates/trust-source/anchors/` (Arch), found by which update tool exists. The tool then rebuilds the system bundle | root (sudo) | `uninstall` |
| DNS delegate for `.lokal` | `/etc/systemd/dns-delegate.d/lokal.dns-delegate` (`DNS=127.0.0.1:53535`, `Domains=~lokal`) | root (sudo) | `uninstall` |
| The lowest port user services may bind | `/etc/sysctl.d/50-lokal.conf` (`net.ipv4.ip_unprivileged_port_start = 80`), and the running kernel's setting | root (sudo) | `uninstall`, which sets 1024 back |
| systemd user units | `~/.config/systemd/user/dev.lokal.daemon.service` and `dev.lokal.daemon-{https,http,dns}.socket` | you | `uninstall` |
| Copy of the lokal binary that the units run | `<base>/bin/lokal` | you | `uninstall` |
| The CA in browsers' NSS databases | `~/.pki/nssdb` (Chrome, Chromium), `~/snap/chromium/current/.pki/nssdb`, and each Firefox profile under `~/.mozilla/firefox`, `~/snap/firefox/common/.mozilla/firefox` and `~/.var/app/org.mozilla.firefox/.mozilla/firefox` | you | `uninstall` |
| Logs, and `lokal up`'s locks, sockets and logs | `~/.local/state/lokal/` (`$XDG_STATE_HOME/lokal`) | you | `uninstall --purge` |

lokal doesn't change `/etc/hosts`, `/etc/nsswitch.conf` or `/etc/resolv.conf`. It reads the last two to check
that programs ask systemd-resolved for names. If they don't, setup says what to install (`libnss-resolve` on
Debian and Ubuntu) and stops before it changes anything.

### Privileged commands on Linux

They are built in `TrustStore::plan_*` (`trust.rs`), `SystemResolver::plan_*` (`resolved.rs`) and, on Linux
only, `ServiceManager::plan_*` (`systemd.rs`), each program by its absolute path. `setup`, on a fresh Ubuntu
26.04 machine (from `lokal setup --dry-run`):

```text
sudo /usr/bin/install -m 0644 <base>/ca/root.pem /usr/local/share/ca-certificates/lokal-<sha1>.crt
sudo /usr/sbin/update-ca-certificates
sudo /usr/bin/install -D -m 0644 <base>/resolver/lokal /etc/systemd/dns-delegate.d/lokal.dns-delegate
sudo /usr/bin/systemctl restart systemd-resolved
sudo /usr/bin/install -m 0644 <base>/resolver/ports.conf /etc/sysctl.d/50-lokal.conf
sudo /usr/sbin/sysctl -w net.ipv4.ip_unprivileged_port_start=80
```

On Fedora and Arch, the CA goes into the p11-kit anchors folder and the update tool is `update-ca-trust`. The last
two commands are planned only if `ip_unprivileged_port_start` is above 80.

`uninstall`:

```text
sudo /usr/bin/rm -f /usr/local/share/ca-certificates/lokal-<sha1>.crt
sudo /usr/sbin/update-ca-certificates
sudo /usr/bin/rm -f /etc/systemd/dns-delegate.d/lokal.dns-delegate
sudo /usr/bin/systemctl restart systemd-resolved
sudo /usr/bin/rm -f /etc/sysctl.d/50-lokal.conf
sudo /usr/sbin/sysctl -w net.ipv4.ip_unprivileged_port_start=1024
```

The last two are planned only if `/etc/sysctl.d/50-lokal.conf` exists. `setup` replacing an outdated CA removes
the old anchor file the same way, in the same sudo batch.

Before it changes anything, setup checks that systemd-resolved is running and supports DNS delegates (systemd 258
or later), that programs ask it for names, and that the system has a trust tool (`SystemResolver::unavailable`,
`TrustStore::unavailable`, SPEC §7.2 and §6.3). You type your password once; there's no second dialog.

After setup, nothing runs as root. The background service is a set of systemd user units that run as you. The
user's systemd binds 443 and 80 on `127.0.0.1` and `::1`, and UDP 53535 on `127.0.0.1`, and passes the sockets
to the service by name (SPEC §12 V12).

### The port setting

A user's systemd units can't bind ports below `net.ipv4.ip_unprivileged_port_start`, 1024 by default ("Failed to
listen on sockets: Permission denied", V12). So setup lowers it to 80: `/etc/sysctl.d/50-lokal.conf` for every
boot, and `sysctl -w` for the running kernel (SPEC §16.6).

The setting is machine-wide. While lokal is set up, **every local user and program can bind ports 80 to 1023**,
not only lokal. `lokal uninstall` removes the file and sets the kernel's default, 1024, back. The alternatives
were a system unit, which would keep the default but need root for `lokal restart` and `lokal stop`, or a
redirect to high ports (SPEC §8.1 (c)).

### Trust and browsers on Linux

- **The system store** (`trust.rs`). The CA is copied into the anchors folder as `lokal-<sha1>.crt`, and the
  update tool rebuilds the bundle that OpenSSL, curl and rustls read (`/etc/ssl/certs/ca-certificates.crt` on
  Debian and Ubuntu, `/etc/pki/tls/certs/ca-bundle.crt` on Fedora). An anchor has no per-use trust setting, so
  there's nothing to narrow to HTTPS, and `ca.trust_scope` doesn't apply (`trust_too_broad` is always false). A
  program that uses the system bundle to check something other than a server's TLS certificate trusts lokal's CA
  for that too. The name constraints still apply. `ca.trusted` verifies a throwaway leaf for
  `lokal-trustcheck.lokal` with rustls against the bundle itself, never `SSL_CERT_FILE`.
- **Browsers** (`nss.rs`). Chrome, Chromium and Firefox don't read the system bundle; they trust what their NSS
  databases hold. Setup adds lokal's CA to each with `certutil -A -t C,,`, as the user and never as root, trusted
  only to issue TLS server certificates, with the nickname `lokal development CA <first 12 hex digits of its
  SHA-1>`. If Chrome is installed but has never run, setup creates `~/.pki/nssdb` first. lokal adds only its own
  CA and changes no browser setting. Without `certutil` (`libnss3-tools`, `nss-tools`), setup and doctor warn.
- **Names.** Linux has no `.local` publisher (SPEC Appendix A), so only `.lokal` names resolve.

### Uninstalling on Linux

`lokal uninstall` follows `install.json`, as on macOS. It stops, disables and removes the systemd units and their
copy of lokal, removes the anchor file of every lokal CA it finds and runs the update tool, removes the DNS
delegate and restarts systemd-resolved, and, if setup lowered it, sets the port setting back to 1024. It removes
every entry whose nickname starts with `lokal development CA` from each NSS database. `--purge` deletes
`~/.local/share/lokal/` and `~/.local/state/lokal/`.
