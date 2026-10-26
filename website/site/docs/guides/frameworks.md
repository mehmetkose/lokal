---
title: Using lokal with your framework
description: Vite and other Host-checking dev servers, servers that detach, debugger ports, hot reload, IPv6-only servers, Docker, streaming, OAuth and cookies.
section: guides
order: 4
sidebarLabel: Frameworks
source: [README.md, docs/TROUBLESHOOTING.md, docs/SPEC.md, docs/ROADMAP.md, crates/lokal/src/cli/args.rs, crates/lokal/src/runner.rs]
---

Most dev servers work behind lokal with no changes: you add a service for the port they listen on, or let
[`lokal run`](#let-lokal-pick-the-port-lokal-run) pick the port and start the server, and open the `https://` URL.
This page covers the cases that need a setting on one side or the other.

## Vite and other dev servers that check the `Host` header

Vite, and some other dev servers, reject requests whose `Host` header they don't recognise. Vite shows
"Blocked request. This host is not allowed". You have two options.

Allow your lokal names in the dev server:

```js
// vite.config.js
export default { server: { allowedHosts: ['.myapp.lokal'] } }
```

The leading dot allows `myapp.lokal` and every name under it. `allowedHosts: true` allows any host.

Or have lokal send `Host: localhost:<port>` upstream instead of the original host:

```console
$ lokal service add web 5173 --host-header upstream
$ lokal service update web --host-header upstream    # for a service you already added
$ lokal run web --host-header upstream -- vite --port {port}   # lokal picks the port and starts Vite
```

The original host is still available in `X-Forwarded-Host`. lokal also sets `X-Forwarded-Proto: https` and
`X-Forwarded-Port: 443`, and appends the client to `X-Forwarded-For`. Other dev servers that check the `Host`
header work the same way.

## Let lokal pick the port: `lokal run`

`lokal run <name> -- <command>` picks a free port, routes the name to it, and starts your dev server on it. The
server learns the port in one of two ways:

- from the `PORT` environment variable, which many servers read by themselves;
- from `{port}` in its arguments, which lokal replaces with the port, for servers that take a flag instead.

| Dev server | Command |
|---|---|
| Next.js (`next dev` reads `PORT`) | `lokal run web -- npm run dev` |
| Express and other Node servers that read `process.env.PORT` | `lokal run api -- node server.js` |
| Vite | `lokal run web --host-header upstream -- vite --port {port}`, or `-- npm run dev -- --port {port}` through npm |
| Rails | `lokal run app -- bin/rails server -p {port}` |
| Django | `lokal run app -- python manage.py runserver 127.0.0.1:{port}` |
| Flask | `lokal run app -- flask run --port {port}` |
| MkDocs | `lokal run docs -- mkdocs serve -a 127.0.0.1:{port}` |

Vite still checks the `Host` header, so either pass `--host-header upstream` as above or allow your lokal names in
`vite.config.js` (see [above](#vite-and-other-dev-servers-that-check-the-host-header)).

If your server ignores both and listens on a port of its own, lokal notices after a few seconds and prints a
`hint:` line that names that port. Point the route there (`lokal service update web --port 5173`), or add
`--port {port}` to the command. If it listens on several ports, for example its own and a
[debugger's](#several-node-or-workers-servers-debugger-ports), the hint lists them and leaves the choice to you.
With [`lokal up`](/docs/reference/commands#lokal-up), the fix goes in `lokal.toml` instead: `port = 5173` for the
service, or `{port}` in its `run`.

Each command also gets `LOKAL_URL`, its own URL, and `LOKAL_URL_<SUB>` for every service in the project, such as
`LOKAL_URL_API=https://api.myapp.lokal`, so one service can find another by name. The
[command reference](/docs/reference/commands#lokal-run) has the full rules.

## Servers that detach themselves

`lokal run` and [`lokal up`](/docs/guides/lokal-up) follow the processes of the command they start: they show its
output, and Ctrl-C stops it and everything it started. So the command must keep the server in the foreground. A
command that starts the server in the background and exits leaves lokal with nothing to show or stop:

- lokal sees the command exit, and `lokal up` reports it as `web exited 0`;
- the server's output doesn't reach lokal or `lokal logs`;
- Ctrl-C doesn't stop the server, so the next `lokal up` can find its port still in use.

### Astro 7 under a coding agent

Astro 7 moves `astro dev` into the background when it detects a coding agent, such as Claude Code. Under an agent,
the service's output says `Dev server running at http://localhost:4321 (pid …)`, and then `lokal up` reports
`web exited 0`. In your own terminal, `astro dev` stays in the foreground, so the file works for you and fails
for the agent.

Set `ASTRO_DEV_BACKGROUND` for the service in `lokal.toml`:

```toml
[services]
web = { run = "pnpm dev", port = 4321, env = { ASTRO_DEV_BACKGROUND = "1" } }
```

With `lokal run`, put it in front: `ASTRO_DEV_BACKGROUND=1 lokal run web -- pnpm astro dev --port {port}`.

`ASTRO_DEV_BACKGROUND` is Astro's own internal variable, not a documented setting: Astro sets it for the copy of
the server it starts in the background. It was checked with Astro 7.3.6, and a later release may change it. With
it set, `astro dev` skips the agent check and stays in the foreground. Astro's lock file stays too:

- `astro dev status` sees the server, and calls it "background";
- a second `astro dev` doesn't start another server;
- `astro dev stop`, which some repos' agent instructions use to restart a server, stops the server lokal runs,
  and lokal reports it as exited.

The documented alternative is `astro dev --ignore-lock`:

```toml
[services]
web = { run = "pnpm astro dev --ignore-lock", port = 4321 }
```

It also keeps the server in the foreground, but writes no lock file. `astro dev status` and `astro dev stop` don't
see the server, and a second `astro dev` starts alongside it. Under an agent, Astro also writes its output as JSON
lines.

### Other commands that detach

Use the foreground form:

| Instead of | Run |
|---|---|
| `pg_ctl start -D <dir>` | `postgres -D <dir>` |
| `docker compose up -d` | `docker compose up` |
| `pm2 start <app>` | `pm2-runtime <app>` |

## Several Node or Workers servers: debugger ports

A dev server's debugger listens on a port of its own, next to the one lokal routes. `wrangler dev` and
Cloudflare's Vite plugin open a V8 inspector on port 9229, or on the next free port if 9229 is taken. Started at
the same moment, as `lokal up` starts them, two of them can both pick 9229, and one dies with
`bind(): Address already in use`. Node's `--inspect` uses 9229 unless you give it a port.

`lokal up` checks the port it gave the server. When that one is free, it doesn't move the service to another
port, which wouldn't help, and after the exit line it prints
`hint: w says a port is in use, but its own port 53840 is free`, with a link to this section.

Give each server its own debugger port:

- **wrangler:** `"dev": { "inspector_port": 9911 }` in `wrangler.jsonc`, or `--inspector-port 9911`.
- **Cloudflare's Vite plugin:** `cloudflare({ inspectorPort: 9912 })` in `vite.config.ts`. Astro's
  `@astrojs/cloudflare` adapter takes the same option.
- **Node:** `--inspect=0`, which picks any free port.

## Hot reload and WebSockets

lokal proxies WebSockets, so hot reload for Vite, Next.js and webpack works through it.

If hot reload doesn't connect, your dev server's client is probably using a hard-coded URL such as
`ws://localhost:5173` instead of the page's own origin. Configure it to use the lokal URL: the page's host, port
443 and `wss://`. Your dev server's documentation names the setting for its WebSocket URL.

## Servers that listen on `::1` or `127.0.0.1` only

Services default to `--host localhost`, and lokal tries both `::1` and `127.0.0.1` for it, then remembers which
one answered. So it doesn't matter which one your server binds:

- Node 17 and later bind `localhost` to `::1` only.
- Python binds to `127.0.0.1`.

If your server listens on some other address, point the service at it:

```console
$ lokal service update api --host 10.0.0.5
```

lokal warns when the host isn't a loopback address, because requests to it leave your Mac.

## Docker

lokal works with ports you publish on your Mac. For a container started with `docker run -p 8080:80 …`, route the
published port:

```console
$ lokal service add app 8080
```

Containers can't resolve lokal's names themselves. A container that publishes port 443 or 80 on your Mac can
conflict with lokal: see [Port 443 or 80 is already in use](/docs/troubleshooting#port-443-or-80-is-already-in-use).

## Streaming, server-sent events and long polling

lokal streams request and response bodies in both directions, without buffering or compressing them. It has no
read or idle timeout on proxied requests. So these work as they do without lokal:

- server-sent events, with each event passed on as soon as it arrives;
- long polling;
- streaming uploads and downloads.

Browsers can talk HTTP/2 to lokal. lokal talks HTTP/1.1 to your dev server, and WebSockets use HTTP/1.1 end to
end. If nothing answers on the upstream port within 2 seconds, lokal returns a `502` page.

## OAuth, cookies and CORS

Each service has its own hostname, so the parts of the web that depend on names behave the way they do in
production:

- **Cookies** that a service sets for its own host stay with that service. Services no longer share `localhost`
  cookies across ports.
- **CORS** sees the same kind of origins as in production: `https://web.myapp.lokal` calling
  `https://api.myapp.lokal`.
- **OAuth redirect URLs** can be real names, such as `https://web.myapp.lokal/auth/callback`. Register that URL
  with your provider. The provider never connects to it: your browser follows the redirect, and your Mac resolves
  the name.
- **HTTPS-only features** work: `Secure` cookies, HTTP/2 and mixed-content rules, with no certificate warnings.

If your app builds absolute URLs from the request (redirects, OAuth callbacks, links in emails), it needs to
know the request came in over HTTPS. lokal sends `X-Forwarded-Proto: https` and `X-Forwarded-Host` for this. Check
your framework's setting for trusting proxy headers.

lokal itself never adds a `Strict-Transport-Security` header to responses.

Webhooks don't work through lokal. Every listener is on loopback, so a provider's servers can't reach your Mac.
Public URLs for a dev stack are on the [roadmap](/docs/roadmap).

## Calling one service from another

Server-side code that calls another service's lokal URL, such as a Node backend fetching
`https://api.myapp.lokal`, has to trust lokal's CA. Start it from a shell where you ran `eval "$(lokal env)"`.
[Trusting lokal from Node, Python and curl](/docs/guides/lokal-env) covers each tool.

## What doesn't work yet

- **Dev servers that only speak HTTPS.** lokal connects to upstreams over plain HTTP.
- **Opening your dev URLs on a phone or another computer.** lokal listens only on loopback, by design.

Both are on the [roadmap](/docs/roadmap).
