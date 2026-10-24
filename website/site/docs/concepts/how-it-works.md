---
title: How lokal works
description: The two phases, the pieces of the background service, and why adding a service needs no sudo and returns only when the URL works.
section: concepts
order: 1
sidebarLabel: How it works
source: [README.md, docs/ARCHITECTURE.md, docs/SPEC.md]
---

lokal is one binary with two modes. As a CLI (`lokal <command>`), it edits your projects and talks to the
background service. As the background service, it answers DNS for `.lokal`, publishes `.local` names, issues
certificates and proxies HTTPS to your dev servers. This page explains how those pieces fit together.

## Two phases

1. **Setup, once per Mac.** `lokal setup` is the only time anything privileged happens. Each of its one-time
   steps removes a privilege that would otherwise be needed at runtime.
2. **Daily use, fast and quiet.** `project` and `service` commands edit a state file and tell the background
   service to reload. They need no sudo, no prompts and no restarts, and they return only when the URL works.

Here's what each setup step buys:

| Problem | One-time step in setup | Result at runtime |
|---|---|---|
| Browsers must trust the certificates | Create a CA as you, and trust it in the System keychain (sudo) | Certificates for your services are issued in memory. The keychain is never touched again |
| Ports 443 and 80 are privileged | Install a LaunchAgent whose plist declares the sockets, so launchd binds them | The background service runs as you and inherits the sockets |
| `*.lokal` names must resolve | Install `/etc/resolver/lokal` (sudo), pointing at `127.0.0.1:53535` | lokal's DNS server answers on a UDP socket that launchd holds |
| `*.local` names must resolve | Nothing | The background service registers local-only records with mDNSResponder, unprivileged |

## The pieces

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

The background service is the same `lokal` binary, run by launchd as `lokal daemon run` from the LaunchAgent
`dev.lokal.daemon`. It's hidden from `lokal --help`, because you never run it yourself.

### Certificates

Setup creates a certificate authority on your Mac, as you, and macOS trusts it. The CA is name-constrained: it
can only sign names under `.lokal` and `.local`, never public domains or IP addresses. The
[security model](/docs/concepts/security-model#the-certificate-authority) covers what that protects.

The background service issues one leaf certificate per configured name, in memory, signed by that CA:

- Each leaf has exactly one name, is valid for 90 days, and is issued again when fewer than 30 days remain.
- Leaves are issued while the service is added (pre-warmed), so the first browser handshake never waits for
  signing. Issuing one takes around a millisecond.
- Nothing is written to disk or to the keychain. A restart issues fresh leaves for every name.
- There are no wildcard certificates. A TLS handshake for a name you didn't configure, or with no SNI, fails.

### Names

`.lokal` and `.local` names resolve in different ways, but both end at loopback and never leave your Mac.

- **`.lokal`.** `/etc/resolver/lokal` sends every `*.lokal` lookup to a small DNS server inside the background
  service, on `127.0.0.1:53535`. It answers every name under `.lokal` with `127.0.0.1` and `::1`, with a TTL of 0,
  so nothing is cached. Whether a name is actually served is decided by the proxy and the certificates: an
  unconfigured name resolves, but its TLS handshake fails.
- **`.local`.** The background service registers an A record (`127.0.0.1`) and an AAAA record (`::1`) for each
  name with mDNSResponder, on its local-only interface, so the names are never announced on your network. The
  AAAA record matters: without it, IPv6 lookups for `.local` fall through to multicast and stall for about five
  seconds.

[`.lokal` or `.local`?](/docs/guides/lokal-vs-local) compares the two from the user's side.

### Ports and socket activation

launchd, not lokal, binds TCP ports 443 and 80 on `127.0.0.1` and `::1`, and UDP port 53535 on `127.0.0.1`. The
LaunchAgent declares the sockets, and the background service receives them from launchd when it starts
(`launch_activate_socket`). This means:

- the background service runs as you and never needs root;
- every listener is on loopback, so nothing is reachable from your network;
- connections that arrive while the service restarts wait in the socket's queue instead of failing.

launchd keeps the service running (`KeepAlive`) and restarts it within a second if it crashes. This
socket-activation approach, where launchd binds the privileged ports for an unprivileged agent, follows
[puma-dev](https://github.com/puma/puma-dev).

### The proxy

The proxy terminates TLS (1.2 and 1.3, with HTTP/2 and HTTP/1.1 offered) and forwards each request to the
service's upstream:

- **Routing.** It routes by the `Host` header, falling back to the SNI. The routing table is swapped atomically
  on each reload, so lookups never wait on a lock.
- **Upstreams.** It talks HTTP/1.1 to your dev server over pooled keep-alive connections. For `localhost`
  upstreams it tries `::1` and `127.0.0.1` in parallel and remembers the winner for 30 seconds, because Node 17+
  binds `localhost` to `::1` only and Python to `127.0.0.1` only.
- **Streaming.** Request and response bodies stream in both directions, with no buffering and no compression.
  WebSockets are upgraded and copied both ways, so hot reload works. Proxied requests have no read timeout, so
  server-sent events and long polling work too.
- **Headers.** It sets `X-Forwarded-Proto: https`, `X-Forwarded-Host` and `X-Forwarded-Port: 443`, and appends the
  client to `X-Forwarded-For`. With `--host-header upstream` it rewrites `Host` to the upstream's `host:port`. It
  never sends `Strict-Transport-Security`.
- **Errors.** If your dev server doesn't accept a connection within 2 seconds, the proxy returns a `502` page
  that names the upstream. Plain HTTP on port 80 gets a `308` redirect to `https://` for configured names, and a
  small `404` page that lists the project's URLs for anything else. If a request's `Host` belongs to a different
  name than the connection's SNI, which can happen when HTTP/2 reuses a connection, the proxy answers `421` so
  the browser retries on a new connection.
- **Connection limits.** A connection that isn't proxying anything is closed 20 seconds after it opens if it
  sends no request, and after 60 idle seconds otherwise. Client and upstream connections are each capped below
  the background service's open-file limit. When the cap is full, the connection idle longest makes room for a
  new one, so idle connections can't lock everyone out. Proxied requests and WebSockets are never cut.

### IPC and the state file

The CLI and the background service share two things:

- **`state.toml`**, your projects and services. Only the CLI writes it, always under a lock and with an atomic
  rename. The background service reads it, and also watches it, so hand edits apply without a CLI command.
- **`lokal.sock`**, a Unix socket with mode 0600 in lokal's data directory. The CLI sends one JSON request per
  connection: `ping`, `status`, `reload` or `selftest`. `lokal status` and `lokal doctor` use it, and every
  change ends with a `reload`.

[Files and paths](/docs/reference/files-and-paths) lists every file involved.

## The fast path

Every command that changes something (`project create`, `project remove`, `service add`, `service update`,
`service remove`) takes the same path:

1. It takes an exclusive lock on `state.toml.lock`, then reads, changes and atomically writes `state.toml`
   (temporary file, fsync, rename).
2. It sends `reload` to the background service. The service compares the new state with the old one, issues the
   new certificates, swaps in the new routes and publishes the new names. It replies only when every affected
   name is ready. For `.local` that means mDNSResponder has confirmed both records. For a removal, it means the
   route is gone and the name is no longer served.
3. It prints the result with the measured time, such as `ready in 8ms`.

That's why `service add` needs no sudo: it only writes a file you own and talks to a process you own. The
keychain, the ports and the resolver were set up once, during setup. And it's why the URL works when the command
returns: the command waits for the real event, not for a timer, so there's no "wait a second and retry". lokal's
live test relies on this. It requests each URL right after the command returns and fails if the first attempt
doesn't work.

If the background service isn't running, or setup hasn't run yet, the command still saves the change and prints
a note saying what to run. If a name can't be made ready, the command exits 1 and says which name failed and why.

## Design decisions

- **Wildcard `.lokal` answers.** mDNSResponder caches any failed lookup for about 75 seconds. If lokal answered
  "no such name" for unknown names, a URL you opened before `service add` would stay broken for over a minute. So
  every `.lokal` name resolves, and "not configured" is enforced at the TLS layer instead.
- **A DNS server for `.lokal`.** mDNSResponder refuses local-only records outside `.local`, so `.lokal` needs its
  own resolver file and DNS server. That's the reason for the second privileged step in setup.
- **Explicit mDNS removal.** Closing the connection to mDNSResponder removes its records asynchronously, so
  lookups could briefly still succeed. The background service removes each record explicitly before it reports
  a removal as done.
- **`ThrottleInterval` 1 in the LaunchAgent.** launchd's default 10-second throttle would delay restarting the
  background service after a crash. With 1, a killed service is back in well under a second.
- **A trust check that can fail.** `security verify-cert -r <root>` makes the root an anchor, so it passes even
  when the keychain doesn't trust it. lokal passes its root as a chain certificate (`-c`) instead, so the check
  passes only when macOS really trusts the CA.

## Read the code

[ARCHITECTURE.md](https://github.com/mehmetkose/lokal/blob/main/docs/ARCHITECTURE.md) maps each piece on this
page to its module in the code, and describes the platform seam that keeps macOS specifics in one place. The
[specification](https://github.com/mehmetkose/lokal/blob/main/docs/SPEC.md) has the full detail, and the
[performance](/docs/concepts/performance) page shows how fast each step is on a real Mac.
