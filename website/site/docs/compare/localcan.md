---
title: lokal vs LocalCan
description: Both give local dev servers HTTPS domains. lokal is free and open source, adds a service in about 14 ms, and keeps every name on your computer.
section: compare
order: 2
sidebarLabel: LocalCan
source: [README.md, docs/SPEC.md, docs/live-test-reports/macos-27.0.1-v0.6.0-full.json]
---

lokal and [LocalCan](https://www.localcan.com) both give your dev servers HTTPS domain names instead of `localhost`
ports. lokal is a free, open-source command-line tool: `.lokal` names that stay on your computer, a service added in
milliseconds, and a `lokal.toml` that starts a repo's whole stack. LocalCan is a closed-source desktop app and CLI
built around paid public URLs, with `.local` names that every device on your network can reach.

Pick lokal if you want local HTTPS that's free, open source, measured to be fast, and private to your computer.
Pick LocalCan if you need public URLs, Windows, or your dev URLs on a phone.

## At a glance

| | lokal | LocalCan |
|---|---|---|
| **Price** | Free | Local `.local` domains work without a license key. Public URLs need a plan: Free (one URL at a time, 60-minute sessions), or paid plans from $10 a month |
| **Source code** | Open source, Apache-2.0 | Closed source |
| **Platforms** | macOS 14 or later, and Linux as a preview | macOS 12 or later and Windows, plus a CLI-only build for Linux |
| **Interface** | Command line | Desktop app and command line |
| **Domain names** | `.lokal`, such as `https://api.shop.lokal` | `.local`, such as `https://my-app.local`, plus public `*.localcan.dev` URLs |
| **Visible on your network** | No: names resolve only on your computer, and every listener is on loopback | Yes: names are advertised over mDNS (Bonjour), and every device on the network can reach them |
| **HTTPS** | Its own CA, created on your computer and limited to `.lokal` and `.local` names | Its own root CA, created on your computer. Its docs don't say whether it's limited to certain names |
| **Password** | Once, during `lokal setup`. Never to add a service | Not stated in its docs |
| **Adding a service** | `lokal service add api 4000`: 13.78 ms, and the URL works when the command returns | In the app, or in a YAML project file followed by `localcan reload` |
| **Published speed numbers** | Yes, from its live test: adding a service, proxy overhead, memory, restart | For public tunnels only. None for local domains |
| **Public URLs** | Not yet. [`lokal share`](/docs/roadmap#lokal-share-public-urls-for-a-dev-stack-through-cloudflare-tunnel) is on the roadmap | Yes, its main feature |
| **Scripting** | `--json` on every list command, exit codes, and `lokal.toml` with `lokal up` in your repo | `--json` on most commands, YAML project files, and an MCP server for coding agents |

## Free and open source

lokal is free. Every feature is in the download: no account, no license key, no trial, and no limit on projects,
services or computers. The code is on [GitHub](https://github.com/mehmetkose/lokal) under Apache-2.0, so you can read
what runs with your password, build it yourself and change it.

```console
$ brew install mehmetkose/tap/lokal
$ lokal setup
```

LocalCan is closed source. Its terms say you may not reverse engineer or modify it, and its GitHub repository holds a
README and images, not the app's code. Its local `.local` domains work without a license key. Public URLs need an
account, and beyond the Free plan they're a subscription:

| LocalCan plan | Price | Public URLs | Custom domains |
|---|---|---|---|
| Free | $0, sign-up with an email address | 1, in 60-minute sessions, 1 GB of traffic a month | None |
| Solo | $10 a month, or $96 a year. 1 user, 1 device | 5 | 2 |
| Pro | $16 a month, or $144 a year. 1 user, 2 devices | 10 | 4 |
| Teams | $15 per seat a month, with 3 seats included | Unlimited | Unlimited |

Paid plans start with a 14-day trial and need no credit card for it. These are LocalCan's prices on 2026-11-02;
[its pricing](https://www.localcan.com/#pricing) has today's.

## Fast

lokal is built to be fast, and it measures it. Its release checklist runs the live test on a real Mac, against the
installed lokal, and the test fails if any target is missed. The run for 0.6.0 measured:

| | lokal 0.6.0 | Target |
|---|---|---|
| `lokal service add`, from start to exit (p95) | 13.78 ms | under 300 ms |
| First request to the new URL | worked on the first try | no retry allowed |
| First lookup of a new name | 1.29 ms | under 20 ms |
| What the proxy adds to a request (p95) | 0.19 ms | under 2 ms |
| Resumed TLS connection (p95) | 2.00 ms | under 10 ms |
| Memory, with 30 services | 18.05 MB | under 30 MB |
| CPU when idle | 0.00% | under 0.5% |
| `lokal restart`, until every name serves again | 95.01 ms | under 500 ms |

Three things make it fast:

- **One small native program.** lokal is written in Rust, and the background service is the same binary as the
  command line, with no app window.
- **No reload step.** `lokal service add` returns once the certificate is issued, the route is in place and the
  name resolves. The URL works the moment the command returns, and there's no file to edit or reload command to
  run.
- **New names work at once.** lokal's DNS server answers every `.lokal` name, with a zero TTL. macOS caches a
  failed lookup for about 75 seconds, so a name you looked up a moment before adding it would otherwise stay
  broken. [`.lokal` names](/docs/guides/lokal-vs-local#how-lokal-names-resolve) explains how.

LocalCan publishes speed numbers for its public tunnels (throughput and time to first byte), but none for its local
proxy or `.local` names, and we haven't measured LocalCan ourselves. So this page doesn't compare local speed head to
head. To measure lokal on your own Mac, see [Performance](/docs/concepts/performance#reproduce-it).

## Names that stay on your computer

lokal's names end in `.lokal` and resolve only on your computer, to `127.0.0.1` and `::1`. Every listener is on
loopback, and no name is announced on your network, so nobody else on the network can see or open your dev
servers. On Linux, `.lokal` names resolve the same way, through systemd-resolved.

LocalCan's names end in `.local`. It advertises them on your network over mDNS (Bonjour), and its FAQ says that all
devices connected to the local network can access them. That's what lets you open a LocalCan URL on a phone, once
you've installed its certificate there. On a shared network, such as an office Wi-Fi, those devices include other
people's. On Linux, LocalCan is CLI-only, and its `.local` names don't resolve automatically.

## HTTPS you can check

lokal creates its certificate authority on your computer, as you, and the key never leaves your user folder. The CA
is name-constrained: it can only sign `.lokal` names (and `.local`, lokal's TLD before 0.6.0), never public domains
or IP addresses, so even a leaked key can't impersonate a real website. On macOS it's trusted for HTTPS only.
[Setup](/docs/setup) lists the only commands lokal runs with sudo, and the
[security model](/docs/concepts/security-model) covers the rest.

LocalCan also creates a root CA unique to your computer, writes its key with owner-only permissions, and has you
trust it through your operating system's dialog. Its documentation doesn't say whether the CA is limited to certain
names.

## Made for scripts and coding agents

lokal is a command-line tool first:

- Every command is safe to re-run, every list command has `--json` output with a stable schema, and exit codes say
  what went wrong.
- A `lokal.toml` in your repo lists its services. `lokal up` routes them and starts every dev server, and
  `lokal down` removes the routes. See [A whole repo with `lokal up`](/docs/guides/lokal-up).
- `lokal run web -- npm run dev` picks a free port, routes it and starts the server.
- [Coding agents](/docs/guides/coding-agents) has a snippet that tells an agent how to use lokal in your repo.

LocalCan has a CLI too, with `--json` on most commands, and an MCP server with 26 tools for coding agents. Its
projects are YAML files in `~/.localcan/projects/`. The daemon doesn't watch them, so you run `localcan reload` after
a change. Keeping project files in your app's own repo is, in its docs' words, "in development for a future
release".

## When LocalCan is the better choice

- **You need public URLs,** to show a client your work or to receive webhooks. That's LocalCan's main feature, and
  lokal doesn't have it yet.
- **You want your dev URLs on a phone** or another computer on the same network. lokal can't do this yet:
  [LAN mode](/docs/roadmap#lan-mode-open-your-dev-urls-on-a-phone) is on the roadmap.
- **You work on Windows, or on macOS 12 or 13.** lokal needs macOS 14 or later, or Linux.
- **You prefer a desktop app** to a command line.
- **You want to inspect and replay requests.** LocalCan has a traffic inspector, and lokal doesn't.
- **Your team wants a shared plan** with single sign-on and an audit log.

## Moving from LocalCan to lokal

Quit LocalCan first: its proxy uses port 443, as lokal does. If another program holds port 443 or 80,
`lokal setup` and `lokal doctor` name it.

```console
$ brew install mehmetkose/tap/lokal
$ lokal setup
```

Then create a project for each LocalCan project, and add its services. A LocalCan service on `my-app.local` that
points at `localhost:3000`, and an API on `localhost:4000`, become:

```console
$ lokal project create my-app
$ lokal service add @ 3000      # https://my-app.lokal
$ lokal service add api 4000    # https://api.my-app.lokal
```

Your URLs change from `.local` to `.lokal`, so update OAuth callback URLs, allowed origins and anything else that
names the old domain. To set up a repo in one step, commit a `lokal.toml` and run `lokal up`
([A whole repo with `lokal up`](/docs/guides/lokal-up)).

## Sources

The facts about LocalCan come from its own website and documentation, checked on 2026-11-02:

- Plans and prices: [pricing](https://www.localcan.com/#pricing), and the prices in
  [LocalCan's ngrok comparison](https://www.localcan.com/blog/best-ngrok-alternative) (updated 2026-10-11).
- Local domains without a license key: [CLI overview](https://www.localcan.com/docs/cli/overview).
- Closed source: [terms](https://www.localcan.com/terms) and the
  [LocalCanApp repository](https://github.com/LocalCan/LocalCanApp).
- Platforms: [download](https://www.localcan.com/download), [installation](https://www.localcan.com/docs/installation)
  and the [changelog](https://www.localcan.com/changelog) (macOS 12 minimum).
- Names and mDNS: [local domains](https://www.localcan.com/docs/domains/overview) and the
  [local domains FAQ](https://www.localcan.com/local-domains).
- Certificates: [certificates](https://www.localcan.com/docs/domains/certificates) and
  [security](https://www.localcan.com/security).
- Project files and reload: [configuration](https://www.localcan.com/docs/configuration/overview).
- MCP server: [AI agents](https://www.localcan.com/docs/ai-agents/mcp).
- Public URLs: [public URLs](https://www.localcan.com/docs/public-urls/overview).
- Tunnel speed numbers: [LocalCan's Tailscale Funnel comparison](https://www.localcan.com/blog/tailscale-funnel-alternative).

lokal's numbers come from its
[0.6.0 live test report](https://github.com/mehmetkose/lokal/blob/main/docs/live-test-reports/macos-27.0.1-v0.6.0-full.json).

LocalCan is a product of its own developers. This page isn't affiliated with or endorsed by them. If something here
is wrong or out of date, [open an issue](https://github.com/mehmetkose/lokal/issues).
