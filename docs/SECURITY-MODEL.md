# Security model

lokal asks for a lot of trust: it adds a certificate authority to your Mac's System keychain and runs a
background service that terminates TLS for you. This document covers what it changes, what it protects, and
what risks remain. To report a vulnerability, see [`SECURITY.md`](../SECURITY.md).

The normative rules are in [`SPEC.md` §16](SPEC.md#16-security-requirements-non-negotiable). This document
explains them.

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

The data directory is created with mode 0700. lokal doesn't touch `/etc/hosts`, `pf`, the Application Firewall,
other launchd jobs or any other keychain item.

## Privileged commands

lokal runs a fixed set of commands as root, and only in `setup` and `uninstall`. It prints each one before it runs
it, and `lokal setup --dry-run` shows them without running anything. All of them are built in
`TrustStore::plan_*` and `SystemResolver::plan_*` (`crates/lokal/src/platform/macos/`) and run through `sudo`
with an argument vector. lokal never runs a shell (`sh -c`) and never builds a command line from strings.

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
  HTTPS only;
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
  (`kDNSServiceInterfaceIndexLocalOnly`), so they are never announced on your network.
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
`.lokal` websites on your Mac, because `/etc/resolver/lokal` sends those lookups to lokal.

`lokal doctor`'s `tld.lokal` check asks a DNS root server whether `.lokal` exists, and **fails** if it does. If
that check ever fails, move your projects to `.local` (`lokal project create <name> --domain <name>.local`).

## Firefox

Firefox keeps its own certificate store. If Firefox is installed, setup adds
`user_pref("security.enterprise_roots.enabled", true);` to `user.js` in each Firefox profile, tagged
`// added by lokal`, and uninstall removes exactly that line.

That preference makes Firefox trust **every** root CA that a user or administrator added to the macOS keychain,
not just lokal's. If you don't want that, delete the line.
lokal then won't work in Firefox, but it keeps working everywhere else.

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
