---
title: Projects and services
description: How lokal names things, and how to create, change and remove the URLs for your dev servers.
section: guides
order: 1
source: [README.md, docs/SPEC.md, crates/lokal/src/cli/args.rs, crates/lokal/src/cli/projects.rs, crates/lokal/src/state/validate.rs]
---

lokal has two things you manage. A **project** owns one domain, such as `shop.lokal`. A **service** is a name
under that domain, routed to a port on your Mac. Every command on this page works without sudo, and every change
is live when the command returns.

```console
$ lokal project create shop                    # https://shop.lokal; becomes the current project
$ lokal service add @ 3000                     # the apex itself: https://shop.lokal → localhost:3000
$ lokal service add api 4000                   # https://api.shop.lokal
$ lokal service add v1.api 4001                # nested names work too
$ lokal service add db-ui 8080 --host 127.0.0.1
$ lokal list                                   # every project, service and URL
```

## Projects

```console
$ lokal project create shop
✓ Created shop.lokal (current project)  (done in 6ms)
  Next: lokal service add api 4000
```

A project gets `<name>.lokal` by default. Use one project per repo; the repo's directory name makes a good
project name. Creating a project also makes it the [current project](#the-current-project).

### Choosing the domain with `--domain`

Pass `--domain` to pick the full domain yourself. It must end in `.lokal` or `.local`:

```console
$ lokal project create store --domain store.acme.lokal   # more than one label is fine
$ lokal project create legacy --domain legacy.local      # .local (mDNS) also works
```

`.lokal` and `.local` names resolve in different ways. [`.lokal` or `.local`?](/docs/guides/lokal-vs-local)
explains the trade-offs.

Other TLDs, including `.test` and `.internal`, are rejected with
`only .lokal and .local are supported (other TLDs could shadow real websites)`. lokal's certificate authority can
only sign `.lokal` and `.local` names, so it couldn't serve them anyway.

Two projects can't overlap: a project's domain can't equal, contain or sit under another project's domain. For
example, `app.lokal` and `api.app.lokal` conflict, but `myapp.lokal` and `app.lokal` don't.

## Services

```console
$ lokal service add api 4000
✓ api.shop.lokal → localhost:4000  (ready in 8ms)
  https://api.shop.lokal
```

The first argument is the subdomain, and the second is the port your dev server listens on.

- **`@` is the apex.** `lokal service add @ 3000` serves `https://shop.lokal` itself.
- **Nested names work.** `lokal service add v1.api 4001` serves `https://v1.api.shop.lokal`.
- **Full names are fine too.** `lokal service add api.shop.lokal 4000` is the same as
  `lokal service add api 4000`.

If nothing is listening on the port yet, lokal adds the route anyway and tells you:

```console
$ lokal service add web 3000
✓ web.shop.lokal → localhost:3000  (ready in 9ms)
  https://web.shop.lokal
  note: nothing is listening on localhost:3000 yet
```

Start your dev server whenever you like. Until then, the URL shows a `502` page.

### Where requests go: `--host`

By default lokal forwards to `localhost:<port>`. That works whether your dev server listens on `127.0.0.1` or
only on `::1`: lokal tries both and remembers which one answered. Node 17 and later bind `localhost` to `::1` only,
and Python binds to `127.0.0.1`, so both work without any flags.

Use `--host` when your server listens on a specific address:

```console
$ lokal service add db-ui 8080 --host 127.0.0.1
```

The host is a hostname or an IP address, with no scheme and no port. A bracketed IPv6 address such as `[::1]` is
accepted. If the host isn't a loopback address, lokal adds the service but prints a warning, because requests then
leave your Mac.

lokal talks to your dev server over plain HTTP. Dev servers that only speak HTTPS aren't supported yet (see the
[roadmap](/docs/roadmap)).

### The `Host` header: `--host-header`

`--host-header` decides which `Host` header your dev server sees:

| Mode | Your dev server sees |
|---|---|
| `preserve` (default) | The original host, such as `Host: api.shop.lokal` |
| `upstream` | The upstream's own address, such as `Host: localhost:5173` |

Use `upstream` for dev servers that reject hosts they don't recognise, such as Vite:

```console
$ lokal service add web 5173 --host-header upstream
```

In both modes, lokal sets `X-Forwarded-Host` to the original host, sets `X-Forwarded-Proto: https` and
`X-Forwarded-Port: 443`, and appends the client to `X-Forwarded-For`.
[Using lokal with your framework](/docs/guides/frameworks) has more on Vite and similar dev servers.

## Let lokal start your dev servers

`service add` routes a name to a server you start yourself. Two commands also start the server, and add its
service for you.

**`lokal run`** starts one dev server on a port lokal picks, and routes the service to it:

```console
$ lokal run web -- npm run dev
✓ https://web.shop.lokal → localhost:51234  npm run dev  (route ready in 12ms)
  … your dev server's output …
✓ ready: https://web.shop.lokal (1.4s)
```

The server gets the port as `PORT`, or as `{port}` in any argument: `lokal run web -- vite --port {port}`. The
service stays when the server exits, so the next `lokal run web` uses the same port. `lokal run` also works on a
service you added with `service add`: it starts the server on that service's port.
[Frameworks](/docs/guides/frameworks#let-lokal-pick-the-port-lokal-run) shows the `{port}` flag for common dev
servers.

**`lokal up`** reads the repo's `lokal.toml`, routes every service in it and starts each one's dev server. Its
services belong to the file: `lokal list` marks them `(lokal.toml)`, and `lokal down` removes them and leaves the
services you added by hand. If the file lists a service you added by hand and gives it the same route,
`lokal up` [adopts it](/docs/guides/lokal-up#services-you-added-before-the-file) instead.
[A whole repo with lokal up](/docs/guides/lokal-up) walks through it.

## The current project

`service` commands, `run` and `open` work on one project. lokal picks it in this order:

1. the project you pass with `-p <project>` (or `--project`);
2. the current project;
3. the only project, if you have exactly one;
4. otherwise the command fails with `no project selected: run 'lokal project use <NAME>' or pass -p <NAME>`.

`project show` follows the same order, but takes the project name as an argument (`lokal project show shop`)
instead of `-p`.

`project create` makes the project current, including when you run it again for a project that already exists.
So does `lokal up`, for the project in its `lokal.toml`. To switch, run `lokal project use`:

```console
$ lokal project use shop
✓ Current project: shop (shop.lokal)
$ lokal service add api 4000 -p legacy         # or pick one for a single command
```

There is one current project for your whole Mac, not one per directory or terminal. If you work in several repos
at once, or a script runs alongside you, pass `-p`. Removing the current project leaves no project selected.

## Ready when it returns

Every command that changes something (`project create`, `project remove`, `service add`, `service update` and
`service remove`) saves the change and then waits for the background service. It returns only when every name it
touched is ready: the route is live, the name resolves and its certificate is issued. For a removal, ready means
the name is no longer served. The time in the output (`ready in 8ms`) is how long the command took.

So there's nothing to wait for and nothing to retry. A script can run `lokal service add` and request the URL on
the next line.

If lokal can't make the change live, the command still saves it and says why:

- **Before you've run `lokal setup`**, it prints
  `note: lokal isn't set up on this Mac yet — run 'lokal setup' to make these URLs work`. The URLs start working
  once [setup](/docs/setup) is done.
- **If the background service isn't running**, it prints
  `note: the background service isn't running — start it with 'lokal start'`.

In both cases the output shows `(saved)` instead of a time. If the background service is running but can't make a
name ready, the command exits with code 1 and lists each name that isn't ready, with the reason.

## Safe to re-run

Setting up a project twice does no harm, so scripts and [coding agents](/docs/guides/coding-agents) can run the
same commands every time:

- **`project create`** for a project that exists makes it the current project and changes nothing else:
  `✓ shop.lokal already exists (current project)`. Passing a different `--domain` is an error.
- **`service add`** with the same port, host and host-header mode changes nothing, and still waits until the
  service is ready: `(already configured, ready in 5ms)`. With different settings it fails and prints the command
  to run instead:

```console
$ lokal service add api 5000
error: service `api` already exists (localhost:4000); use `lokal service update api --port 5000`
```

## List, show and open

```console
$ lokal list                    # every project with its services
$ lokal project list            # one line per project
$ lokal project show            # one project: the current one, or the one you name
$ lokal service list            # the current project's services (-p to pick another)
$ lokal open api                # open https://api.<project>.lokal in your browser
```

`lokal list` shows each project's domain, then a line per service with its URL and where it goes:

```console
$ lokal list
shop  shop.lokal  (current)
  @       https://shop.lokal         → localhost:3000
  api     https://api.shop.lokal     → localhost:4000
  db-ui   https://db-ui.shop.lokal   → 127.0.0.1:8080
  v1.api  https://v1.api.shop.lokal  → localhost:4001
```

All four list commands take `--json`, a stable, machine-readable format described in
[Using lokal with coding agents](/docs/guides/coding-agents#machine-readable-output). `lokal open` takes `-p` too,
and `lokal open @` opens the apex.

## Change and remove

`service update` changes a service's port, host or host-header mode. Pass at least one of `--port`, `--host` and
`--host-header`:

```console
$ lokal service update api --port 4002
✓ api.shop.lokal → localhost:4002  (ready in 7ms)
$ lokal service update web --host-header upstream
$ lokal service remove api
✓ Removed api.shop.lokal  (done in 4ms)
```

`service remove` takes several names and removes them in one change. If any of them doesn't exist, it removes
nothing:

```console
$ lokal service remove web docs
✓ Removed web.shop.lokal
✓ Removed docs.shop.lokal  (done in 5ms)
```

`project remove` refuses to delete a project that still has services. Remove them first, or pass `--force` to
remove the project and its services together:

```console
$ lokal project remove shop --force
```

You can't change a project's domain in place. To move a project to another domain, remove it and create it again
with the new `--domain`, then add its services back.

## Name rules

lokal trims and lowercases every name before it checks it, so `MyApp` becomes `myapp`. A name that breaks a rule
fails with exit code 2 and says what's wrong.

| What | Rule |
|---|---|
| Label (each part of a name) | 1 to 63 characters: letters, digits and `-`, not starting or ending with `-` |
| Project name | A single label. No dots: use `--domain` for a longer domain |
| Domain (`--domain`) | At least two labels, at most 253 characters, ending in `.lokal` or `.local` |
| Subdomain | `@`, or labels joined by dots (`v1.api`). The full name can be at most 253 characters |
| Port | 1 to 65535. Not 443 or 80 on a loopback host: those are lokal itself, so requests would loop back into it |
| Host | A hostname or IP address, with no scheme (`http://`) and no port. Default `localhost` |

Subdomains are unique within a project.

## Aliases

| Command | Short form |
|---|---|
| `lokal project …` | `lokal p …` |
| `lokal service …` | `lokal s …` |
| `lokal list` | `lokal ls` |
| `project create`, `service add` | `new` |
| `project list`, `service list` | `ls` |
| `project remove`, `service remove` | `rm` |
| `project use` | `switch` |
| `service update` | `set` |

So `lokal s new api 4000` is `lokal service add api 4000`, and `lokal p switch shop` is `lokal project use shop`.
The [command reference](/docs/reference/commands) lists every command and flag.
