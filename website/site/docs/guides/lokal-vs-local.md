---
title: ".lokal names"
description: How lokal's .lokal names resolve, why .local names were removed in 0.6.0, and how to move a project that is still on .local.
section: guides
order: 7
sidebarLabel: ".lokal names"
source: [README.md, CHANGELOG.md, docs/SPEC.md, docs/SECURITY-MODEL.md, docs/TROUBLESHOOTING.md]
---

Every name lokal serves ends in `.lokal`. `lokal project create shop` gives you `shop.lokal`, and `--domain` lets you
pick another name, as long as it ends in `.lokal` (`shop.acme.lokal`). Names resolve the same way on macOS and
Linux, and they never leave your computer.

## How `.lokal` names resolve

The background service includes a small DNS server on `127.0.0.1:53535`. [Setup](/docs/setup) tells your system
to send every `*.lokal` lookup to it:

- **On macOS**, with `/etc/resolver/lokal`.
- **On Linux**, with a systemd-resolved DNS delegate, `/etc/systemd/dns-delegate.d/lokal.dns-delegate`. It needs
  systemd 258 or later, and it works for programs that ask systemd-resolved for names, which setup checks before
  it changes anything.

The server answers every name under `.lokal` with `127.0.0.1` and `::1`, with a zero TTL so nothing is cached,
and refuses names outside `.lokal`.

It answers *every* name, including ones you haven't configured, on purpose. macOS caches failed lookups for about
75 seconds. If lokal said "no such name", a name you looked up a moment before `lokal service add` would stay broken
for over a minute. Because no lookup ever fails, a new name works the moment the command returns.

Whether a name is actually served is decided by the proxy instead. A name is served when it has a route and a
certificate. An unconfigured `.lokal` name resolves to loopback, but lokal has no certificate for it, so the HTTPS
handshake fails. Over plain HTTP it gets a `404` page.

## `.local` names were removed in 0.6.0

Before 0.6.0, lokal also served `.local` names, on macOS only. They didn't go through DNS: the background service
registered each name with macOS's mDNSResponder (Bonjour) as local-only records. Linux never had that, because it
has no equivalent of local-only mDNS records.

0.6.0 removed them, so lokal resolves names one way on every system, with less code to keep safe. New `.local`
domains are refused:

```console
$ lokal project create shop --domain shop.local
error: .local names aren't supported since 0.6.0
hint: use --domain shop.lokal
```

`domain = "shop.local"` in a `lokal.toml` fails the same way, at its line and column, with the hint
`set domain = "shop.lokal"`.

lokal's certificate authority still covers `.local` as well as `.lokal`. Narrowing it would replace the CA on every
machine, with a password prompt. So upgrading needs no new `lokal setup` and no password.

## Projects still on `.local`

A project that an earlier lokal created on `.local` still loads, but it isn't served: its URLs stop working. lokal
tells you where it can:

- `lokal status`, `lokal list` and `lokal project list` end with a warning that names it and the fix, and
  `project list` marks it `(not served)`.
- `lokal doctor` reports it as a `project.<name>` warning, instead of checks that could only fail.
- `lokal service add`, `lokal service update`, `lokal run` and `lokal up` refuse it.
- `lokal project remove` and `lokal service remove` work on it as on any other project.

```console
$ lokal status
…
  warning: shop.local isn't served: lokal serves only .lokal names since 0.6.0
    move it to .lokal: lokal project remove shop --force, then lokal project create shop and add its services again (or run lokal up, if its repo has a lokal.toml)
```

lokal never moves a project to `.lokal` by itself, because a new URL breaks OAuth callbacks, cookies and
bookmarks. You choose when to move it.

## Moving a project to `.lokal`

A project's domain can't be changed in place. Remove the project and create it again, then add its services back:

```console
$ lokal project remove shop --force
$ lokal project create shop        # shop.lokal
$ lokal service add api 4000       # add each service again
```

If the project had a custom domain such as `shop.acme.local`, pass the same name under `.lokal`:
`lokal project create shop --domain shop.acme.lokal`.

In a repo with a [`lokal.toml`](/docs/guides/lokal-up), remove the project and run `lokal up` again. If the file
sets `domain = "shop.local"`, change it to `domain = "shop.lokal"` first.

Then update anything that still points at the old URLs, such as OAuth redirect URLs and `.env` files.

## The `.lokal` TLD

`.lokal` isn't a reserved top-level domain. It isn't a real one in the public DNS today, but in theory it could
become one. If that happened, lokal would hide real `.lokal` websites on your computer, because your system sends
those lookups to lokal.

`lokal doctor` guards against this. Its `tld.lokal` check asks a DNS root server whether `.lokal` exists, and
**fails** if it does. It's skipped when you're offline. This query is the only network traffic lokal sends off
your computer. If that check ever fails, `lokal uninstall` removes the resolver config, and the maintainers want
to hear about it: [open an issue](https://github.com/mehmetkose/lokal/issues). The
[security model](/docs/concepts/security-model#the-lokal-tld) has the full picture.

## Why not `.test` or `.internal`?

lokal's certificate authority is trusted system-wide, so its name constraints are as short as they can be: `.lokal`,
and `.local` from before 0.6.0. Allowing `.test` or `.internal` too would let a leaked key sign names that companies
use for real internal hosts. So `--domain` accepts only `.lokal`, and rejects other TLDs with
`only .lokal is supported (other TLDs could shadow real websites)`.
