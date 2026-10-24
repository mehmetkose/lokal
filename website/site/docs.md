---
title: Real HTTPS domains for your local dev servers
description: lokal gives every dev server on your Mac its own .lokal name with a certificate your tools trust.
section: start
order: 1
sidebarLabel: Introduction
source: [README.md, CHANGELOG.md]
---

lokal is an open-source command-line tool for macOS. It gives each dev server you run its own domain with valid
HTTPS: `https://api.myapp.lokal` instead of `http://localhost:4000`, with a certificate that your browser, curl,
Node and Python trust. You set it up once. After that, adding a service takes milliseconds and never asks for a
password.

```console
$ lokal setup                        # once per Mac; asks for your password
$ lokal project create myapp         # claims myapp.lokal
$ lokal service add api 4000
✓ api.myapp.lokal → localhost:4000  (ready in 8ms)
  https://api.myapp.lokal
$ curl https://api.myapp.lokal       # works: no -k, no --cacert
```

When a command returns, the URL already works: the name resolves, the certificate exists and the route is live.
There's nothing to wait for and nothing to retry.

## Before and after

| | `http://localhost:4000` | `https://api.myapp.lokal` |
|---|---|---|
| Hostname | Every service shares `localhost` and differs only by port | Every service has its own name |
| Cookies | Shared across ports | Scoped to the service, like in production |
| HTTPS | None, or `-k`, `--insecure` and browser warnings | A valid certificate, trusted by macOS |
| CORS, OAuth redirects, subdomain routing | Behave differently from production | Behave the way they do in production |

## Why lokal

- **Real names.** Every service gets its own hostname, so cookies, CORS, OAuth redirect URLs and subdomain
  routing behave the way they do in production. Services no longer share `localhost` cookies across ports.
- **Valid HTTPS.** Secure cookies, HTTP/2 and mixed-content rules work like they do in production, without `-k`,
  `--insecure` or clicking through browser warnings.
- **Fast and quiet.** Adding, changing or removing a service needs no sudo, no prompts and no restarts.
- **Nothing leaves your Mac.** Every listener is on loopback, and names are never announced on your network.
- **A narrowly scoped certificate authority.** lokal's CA can only sign `.lokal` and `.local` names, never public
  domains or IP addresses. Even a leaked key can't impersonate a real website.
- **It handles your whole dev stack.** WebSockets (Vite, Next.js and webpack hot reload), server-sent events, long
  polling, streaming uploads and HTTP/2 all pass through. `localhost` upstreams work whether your server listens
  on `127.0.0.1` or only on `::1`.
- **Scripts and coding agents can drive it.** Every command is safe to re-run, every list command has a stable
  `--json` output, and exit codes say what went wrong.

Browsers and most macOS tools use the keychain, so they trust lokal as soon as setup finishes. Node, Python and
some other tools ship their own certificate bundles. One line, [`eval "$(lokal env)"`](/docs/guides/lokal-env),
points them at lokal's CA.

## Requirements and status

lokal is new (v0.1) and runs on **macOS only**:

- **macOS 14 Sonoma or later.** lokal is developed and tested on macOS 27. Older versions should work but haven't
  been tested yet.
- **Apple Silicon.** Intel builds are published, but they haven't been tested on an Intel Mac yet.
- **Linux and Windows aren't supported yet.** The code is structured so they can be added later. Linux support is
  on the [roadmap](/docs/roadmap), and
  [`PORTING.md`](https://github.com/mehmetkose/lokal/blob/main/docs/PORTING.md) collects what each OS needs.

If you run lokal on an Intel Mac or an older macOS version, reports are welcome in the
[issue tracker](https://github.com/mehmetkose/lokal/issues).

lokal is free and open source, licensed under Apache-2.0. The code is on
[GitHub](https://github.com/mehmetkose/lokal).

## Where to next

- [Install lokal](/docs/installation) with Homebrew, a prebuilt binary or from source.
- [Set it up](/docs/setup) once: what `lokal setup` changes, and the only commands it runs with sudo.
- [Quick start](/docs/quick-start): create a project, add services and open them, in five minutes.
- [Projects and services](/docs/guides/projects-and-services): apex domains, nested names, other hosts.
- [Frameworks](/docs/guides/frameworks): Vite and other dev servers that check the `Host` header.
- [Coding agents](/docs/guides/coding-agents): let an agent set up routes for your repo.
- [How it works](/docs/concepts/how-it-works) and the [security model](/docs/concepts/security-model).
- [Command reference](/docs/reference/commands): every command and flag.
- Something broken? Start with [troubleshooting](/docs/troubleshooting) and the [FAQ](/docs/faq).
