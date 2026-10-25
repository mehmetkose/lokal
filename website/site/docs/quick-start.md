---
title: Quick start
description: Create a project, route your dev servers to real HTTPS names, and manage them, in five minutes.
section: start
order: 4
source: [README.md, crates/lokal/src/cli/projects.rs, crates/lokal/src/cli/system.rs]
---

This walkthrough takes about five minutes. It assumes you've [installed lokal](/docs/installation) and run
[`lokal setup`](/docs/setup). None of the commands below need sudo or ask for a password, and each URL works as
soon as its command returns.

## 1. Create a project

A **project** owns one domain. Create one per repo. The repo's directory name makes a good project name.

```console
$ lokal project create shop
✓ Created shop.lokal (current project)  (done in 9ms)
  Next: lokal service add api 4000
```

The new project becomes the **current project**, so the `service` commands below use it without any extra flags.
Project names use letters, digits and `-`. Running `project create` again for a project that exists is safe: it
makes that project current again.

New projects get `.lokal` names. To give a project a `.local` domain instead, pass `--domain` when you create it,
for example `lokal project create legacy --domain legacy.local`.
[`.lokal` or `.local`?](/docs/guides/lokal-vs-local) explains the difference.

## 2. Add services

Each **service** is a subdomain routed to a port on your Mac. Start your dev server, then route a name to its port:

```console
$ lokal service add api 4000
✓ api.shop.lokal → localhost:4000  (ready in 8ms)
  https://api.shop.lokal
$ curl https://api.shop.lokal        # works: no -k, no --cacert
```

If nothing is listening on the port yet, lokal still adds the route and tells you:

```console
$ lokal service add @ 3000           # the apex itself: https://shop.lokal
✓ shop.lokal → localhost:3000  (ready in 7ms)
  https://shop.lokal
  note: nothing is listening on localhost:3000 yet
```

A few more forms:

```console
$ lokal service add v1.api 4001                   # nested names work too: https://v1.api.shop.lokal
$ lokal service add db-ui 8080 --host 127.0.0.1   # an upstream host other than localhost
```

- **`@`** routes the project's apex, `https://shop.lokal`.
- **Nested names** such as `v1.api` give you `https://v1.api.shop.lokal`.
- **`--host`** sends requests to a host other than `localhost`. You rarely need it: `localhost` already works
  whether your server listens on `127.0.0.1` or only on `::1`. lokal warns if the host isn't loopback.

Adding the same service again changes nothing. Adding an existing name with a different port fails and prints the
`lokal service update` command to use instead.

### Or let lokal pick the port

`lokal run` routes a service and starts its dev server in one step. It picks a free port, routes the service to
it, and starts the server on it:

```console
$ lokal run web -- npm run dev
✓ https://web.shop.lokal → localhost:51234  npm run dev  (route ready in 12ms)
  … your dev server's output …
✓ ready: https://web.shop.lokal (1.4s)
```

The server gets the port as `PORT`. For servers that ignore `PORT`, put `{port}` in the command:
`lokal run web -- vite --port {port}`. Ctrl-C stops the server. The route stays, and the next `lokal run web` uses
the same port.

For a repo with several dev servers, list them in a `lokal.toml` and start them all with `lokal up`.
[A whole repo with lokal up](/docs/guides/lokal-up) shows how.

## 3. See what you have

`lokal list` shows every project with its services and URLs:

```console
$ lokal list
shop  shop.lokal  (current)
  @       https://shop.lokal         → localhost:3000
  api     https://api.shop.lokal     → localhost:4000
  db-ui   https://db-ui.shop.lokal   → 127.0.0.1:8080
  v1.api  https://v1.api.shop.lokal  → localhost:4001
  web     https://web.shop.lokal     → localhost:51234
```

`lokal list --json` prints the same thing in a stable, machine-readable format for scripts and coding agents.

## 4. Open, change and remove services

```console
$ lokal open api                      # open https://api.shop.lokal in your default browser
✓ opened https://api.shop.lokal
$ lokal service update api --port 4002
✓ api.shop.lokal → localhost:4002  (ready in 7ms)
$ lokal service remove api
✓ Removed api.shop.lokal  (done in 6ms)
```

`service`, `run`, `project show` and `open` use the current project. Pass `-p <project>` to pick another one, or switch
with `lokal project use <project>`.

## 5. Make Node, Python and curl trust lokal

Browsers and macOS's own tools, `curl` included, trust lokal as soon as setup finishes. Node, Python and some
other tools ship their own certificate bundles. `lokal env` prints the exports that point them at lokal's CA:

```console
$ lokal env
export NODE_EXTRA_CA_CERTS='/Users/you/Library/Application Support/lokal/ca/root.pem'
export SSL_CERT_FILE='/Users/you/Library/Application Support/lokal/ca/bundle.pem'
export REQUESTS_CA_BUNDLE='/Users/you/Library/Application Support/lokal/ca/bundle.pem'
export CURL_CA_BUNDLE='/Users/you/Library/Application Support/lokal/ca/bundle.pem'
```

Load them into your shell before you start the tool:

```console
$ eval "$(lokal env)"                 # zsh, bash
$ lokal env --shell fish | source     # fish
$ node -e "fetch('https://api.shop.lokal').then(r => console.log(r.status))"
$ python3 -c "import urllib.request; print(urllib.request.urlopen('https://api.shop.lokal').status)"
```

The bundle holds the macOS system roots plus lokal's CA, so public HTTPS sites keep working. To make it permanent,
add the `eval` line to your shell profile. [lokal env](/docs/guides/lokal-env) explains each variable.

## Next steps

- [Projects and services](/docs/guides/projects-and-services): everything the `project` and `service` commands
  can do.
- [A whole repo with `lokal up`](/docs/guides/lokal-up): list your dev servers in a `lokal.toml`, and start them
  all with one command.
- [Frameworks](/docs/guides/frameworks): Vite and other dev servers that reject unknown `Host` headers.
- [Coding agents](/docs/guides/coding-agents): tell an agent how to use lokal in your repo.
- [Command reference](/docs/reference/commands): every command and flag.
- Something not working? Run `lokal doctor`, then see [troubleshooting](/docs/troubleshooting).
