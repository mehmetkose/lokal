---
title: FAQ
description: Short answers to common questions about lokal, from mkcert and Docker to platforms and the license.
section: help
order: 2
source: [README.md, docs/SECURITY-MODEL.md, docs/ROADMAP.md, Cargo.toml]
---

Short answers to the questions people ask most. If yours isn't here, [troubleshooting](/docs/troubleshooting)
covers specific errors, and you can ask in the [issue tracker](https://github.com/mehmetkose/lokal/issues).

## How is this different from mkcert?

mkcert creates locally trusted certificates. You still have to configure DNS (or `/etc/hosts`) and a server or
proxy for each name. lokal does names, certificates and routing together, and its CA is name-constrained, so it
can only sign `.lokal` and `.local` names.

## Does it work with Docker?

Yes, for ports you publish on your Mac. With `docker run -p 8080:80 …`, run `lokal service add app 8080`.
Containers can't resolve lokal's names themselves.

## Can I open my dev URLs on my phone or another computer?

Not yet. lokal listens only on loopback by design. A LAN mode is on the [roadmap](/docs/roadmap).

## Does it edit `/etc/hosts`?

No. `lokal doctor` warns if `/etc/hosts` has an entry that conflicts with a lokal name.

## Why only `.lokal` and `.local`, and not `.test` or `.internal`?

The CA is trusted system-wide, so it's constrained to exactly the TLDs lokal serves. Adding `.test` or `.internal`
would let a leaked key sign names that companies use for real internal hosts. The
[security model](/docs/concepts/security-model) explains the name constraints.

## Why does setup ask for my password twice?

On macOS, once is sudo in Terminal. The second time is a macOS dialog, because macOS asks separately before it
changes certificate trust settings. On Linux, setup asks once, for sudo. [Setting up lokal](/docs/setup) lists the
exact commands setup runs with sudo.

## Is it macOS only?

Mostly. On macOS, lokal needs macOS 14 Sonoma or later. It's tested on Apple Silicon. Intel builds are
published, but they haven't been tested on an Intel Mac yet. Linux is a preview: since 0.5.1 there are `.deb`
and `.rpm` packages and tarballs for x86_64 and arm64, setup needs systemd 258 or later (such as Ubuntu 26.04 or
Fedora 43), and only `.lokal` names work. See [Linux (preview)](/docs/installation#linux-preview). There are no
Windows builds yet.
[`PORTING.md`](https://github.com/mehmetkose/lokal/blob/main/docs/PORTING.md) collects what each OS needs.

## What's the license?

lokal is licensed under the [Apache License, Version 2.0](https://github.com/mehmetkose/lokal/blob/main/LICENSE).
Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in lokal, as
defined in the Apache-2.0 license, is licensed the same way, without any additional terms or conditions.
