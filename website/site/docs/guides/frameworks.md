---
title: Using lokal with your framework
description: Vite and other Host-checking dev servers, hot reload, IPv6-only servers, Docker, streaming, OAuth and cookies.
section: guides
order: 3
sidebarLabel: Frameworks
source: [README.md, docs/TROUBLESHOOTING.md, docs/SPEC.md, docs/ROADMAP.md, crates/lokal/src/cli/args.rs]
---

Most dev servers work behind lokal with no changes: you add a service for the port they listen on, and open the
`https://` URL. This page covers the cases that need a setting on one side or the other.

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
```

The original host is still available in `X-Forwarded-Host`. lokal also sets `X-Forwarded-Proto: https` and
`X-Forwarded-Port: 443`, and appends the client to `X-Forwarded-For`. Other dev servers that check the `Host`
header work the same way.

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
