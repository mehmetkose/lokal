---
title: lokal compared
description: How lokal compares with other tools that give local dev servers HTTPS domains, on price, speed, names, setup and scripting.
section: compare
order: 1
sidebarLabel: Overview
source: [README.md, docs/live-test-reports/macos-27.0.1-v0.6.0-full.json]
---

Several tools give your local dev servers a domain with HTTPS. These pages compare lokal with them, one tool per
page, on the same points each time, so you can pick the one that fits how you work.

## Comparisons

| Tool | What it is | Comparison |
|---|---|---|
| LocalCan | A closed-source desktop app and CLI for macOS and Windows, with `.local` names on your network and paid public URLs. | [lokal vs LocalCan](/docs/compare/localcan) |
| mkcert | A command-line tool that creates locally trusted certificates. You set up the names and a proxy yourself. | [In the FAQ](/docs/faq#how-is-this-different-from-mkcert) |

## lokal in short

- **Free and open source.** lokal is licensed under Apache-2.0. There's no paid tier, no account, no license key
  and no trial: every feature is in the free download.
- **Fast.** Adding a service takes milliseconds and never asks for a password, and its URL works the moment the
  command returns. For the 0.6.0 release, `lokal service add` took 13.78 ms at the 95th percentile, and the proxy
  added 0.19 ms to a request. [Performance](/docs/concepts/performance) explains the measurements.
- **Nothing leaves your computer.** Names end in `.lokal`, resolve to `127.0.0.1` and `::1` only, and are never
  announced on your network.
- **Made for the command line.** Every command is safe to re-run, every list command has `--json` output, and a
  `lokal.toml` in your repo starts the whole stack with `lokal up`. It suits scripts and
  [coding agents](/docs/guides/coding-agents).
- **macOS and Linux.** macOS 14 or later, and Linux with systemd 258 or later as a preview.

## What each comparison covers

Every comparison page starts with a table of these points, then explains the differences that matter:

- **Price and license.** What the tool costs, what's free, and whether its source is open.
- **Speed.** How long adding a service takes, what the proxy adds to a request, and how much memory it uses.
  lokal's numbers come from its live test, which is part of its release checklist. Another tool's numbers appear
  only if it publishes them or both tools were measured on the same machine, and the page says which.
- **Platforms.** The operating systems, and whether it's a command-line tool or an app.
- **Names.** Which domain names it uses, how they resolve, and whether other devices on your network can see them.
- **HTTPS.** How the certificate authority is created and trusted, and which names it can sign.
- **Setup.** What needs your password, and what runs in the background.
- **Scripting and coding agents.** The command line, machine-readable output, and config files in your repo.
- **Sharing.** Public URLs, and opening your dev URLs from a phone or another computer.

## How these pages are written

- Facts about another tool come from its own website and documentation. Each page lists its sources and the date
  they were checked, and products change, so check them for anything that decides your choice.
- Every page says where the other tool is the better choice, including what lokal doesn't do yet.
- lokal's facts come from these docs and the [lokal repository](https://github.com/mehmetkose/lokal).

If something on these pages is wrong or out of date, [open an issue](https://github.com/mehmetkose/lokal/issues).
