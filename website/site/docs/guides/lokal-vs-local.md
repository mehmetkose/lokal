---
title: ".lokal or .local?"
description: lokal serves two TLDs that resolve in different ways. Here's how each works and when to pick which.
section: guides
order: 6
sidebarLabel: ".lokal or .local"
source: [README.md, docs/SPEC.md, docs/SECURITY-MODEL.md, docs/TROUBLESHOOTING.md]
---

New projects get `.lokal` names. `.local` names keep working through `--domain <name>.local`. Both get the same
certificates, the same proxy and the same commands. The difference is how macOS finds out that a name points at
your Mac.

## At a glance

| | `.lokal` (default) | `.local` |
|---|---|---|
| Resolved by | lokal's own DNS server, through `/etc/resolver/lokal` | macOS's mDNSResponder (Bonjour), as local-only records |
| New names | Work at once, even if you looked the name up before adding it | Work at once |
| Unknown names | Resolve to loopback, but get no certificate (and a 404 over plain HTTP) | Take about 5 seconds to fail (the standard multicast timeout) |
| Reserved TLD | No (see [the `.lokal` TLD](#the-lokal-tld)) | Yes, for multicast DNS (RFC 6762) |

Either way, names never leave your Mac.

## How `.lokal` names resolve

[Setup](/docs/setup) installs `/etc/resolver/lokal`, which tells macOS to send every `*.lokal` lookup to lokal's
built-in DNS server on `127.0.0.1:53535`. That server is part of the background service. It answers every name
under `.lokal` with `127.0.0.1` and `::1`, with a zero TTL so nothing is cached, and refuses names outside
`.lokal`.

It answers *every* name, including ones you haven't configured, on purpose. macOS caches failed lookups for about
75 seconds. If lokal said "no such name", a name you looked up a moment before `lokal service add` would stay broken
for over a minute. Because no lookup ever fails, a new name works the moment the command returns.

Whether a name is actually served is decided by the proxy instead. An unconfigured `.lokal` name resolves to
loopback, but lokal has no certificate for it, so the HTTPS handshake fails. Over plain HTTP it gets a `404` page.

## How `.local` names resolve

`.local` names don't go through DNS. The background service registers each configured name with macOS's
mDNSResponder, the Bonjour service, as an `A` record for `127.0.0.1` and an `AAAA` record for `::1`. It registers
them on the local-only interface, so they're never announced on your network.

The records exist only while the background service runs. Names you haven't configured aren't registered, so
macOS asks the network for them, and they take about 5 seconds to fail.

A custom TLD can't work this way: mDNSResponder accepts local-only records only under `.local`. That's why
`.lokal` needs the resolver file and its own DNS server.

## The `.lokal` TLD

`.local` is reserved for multicast DNS (RFC 6762). `.lokal` isn't reserved. It isn't a real top-level domain in the
public DNS today, but in theory it could become one. If that happened, lokal would hide real `.lokal` websites on
your Mac, because `/etc/resolver/lokal` sends those lookups to lokal.

`lokal doctor` guards against this. Its `tld.lokal` check asks a DNS root server whether `.lokal` exists, and
**fails** if it does. It's skipped when you're offline. This query is the only network traffic lokal sends off
your Mac.

If that check ever fails, move your projects to `.local`. The
[security model](/docs/concepts/security-model#the-lokal-tld) has the full picture.

## Which one to choose

Use `.lokal` unless you have a reason not to. New names work immediately, and unknown names fail fast instead of
hanging for 5 seconds.

Use `.local` in two cases:

- **A VPN or DNS-filtering app takes over all DNS resolution** on your Mac, and `.lokal` names stop resolving while
  it's active. `.local` names don't go through DNS, so they keep working.
- **`lokal doctor` fails `tld.lokal`**, because `.lokal` has become a real TLD.

## Using `.local`

Pass the full domain when you create the project:

```console
$ lokal project create legacy --domain legacy.local
$ lokal service add api 4000 -p legacy             # https://api.legacy.local
```

Services, nested names and the `@` apex work exactly as they do under `.lokal`.

A project's domain can't be changed in place: running `lokal project create` again with a different `--domain`
fails. To move a project from `.lokal` to `.local`, remove it and create it again, then add its services back:

```console
$ lokal project remove shop --force
$ lokal project create shop --domain shop.local
$ lokal service add api 4000
```

## Why not `.test` or `.internal`?

lokal's certificate authority is trusted system-wide, so its name constraints allow exactly the TLDs lokal serves:
`.lokal` and `.local`. Allowing `.test` or `.internal` too would let a leaked key sign names that companies use for
real internal hosts. So `--domain` accepts only `.lokal` and `.local`, and rejects anything else with
`only .lokal and .local are supported (other TLDs could shadow real websites)`.
