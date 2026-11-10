---
title: "Routes: one server, many sites"
description: Give every site that one dev server renders a .lokal name of its own, with a host pattern in lokal.toml that forwards to a path.
section: guides
order: 3
sidebarLabel: Routes
source: [README.md, CHANGELOG.md, docs/SPEC.md, crates/lokal/src/cli/args.rs, crates/lokal/src/project_file.rs, crates/lokal/src/state/validate.rs, crates/lokal/src/proxy/routes.rs]
---

Some dev servers render many sites from one process, each under a path of its own: a site builder serves
`/demosite/acme.com`, a preview server serves `/preview/<slug>`. In production, an edge (a worker, nginx, Caddy)
gives each site its own host and maps it to that path. A route in `lokal.toml` does the same on your Mac, so each
site gets a `.lokal` name, and its root-relative links, its cookies and `location.host` behave as they will in
production.

## Add a route

Routes go in a `[routes]` table in [`lokal.toml`](/docs/guides/lokal-up), next to the services. Each key is a host
pattern, and each value is the URL to send it to:

```toml
project = "seopages"

[services]
render = "npm run dev -- --port {port}"

[routes]
"{site}" = "https://render.seopages.lokal/demosite/{site}"
```

`lokal up` writes the route along with the services, and prints a line for it:

```console
$ lokal up
✓ https://render.seopages.lokal → localhost:51234  npm run dev -- --port 51234
✓ https://{site}.seopages.lokal → https://render.seopages.lokal/demosite/{site}
  routes ready in 38ms
```

`{site}` takes whatever comes before `seopages.lokal`, and lokal puts it into the path:

| You open | render gets |
|---|---|
| `https://acme.com.seopages.lokal/` | `/demosite/acme.com` |
| `https://acme.com.seopages.lokal/about?x=1` | `/demosite/acme.com/about?x=1` |
| `https://blog.example.org.seopages.lokal/` | `/demosite/blog.example.org` |

Each site's name gets its own certificate on its first visit, naming exactly that name, as a name under a
[wildcard service](/docs/guides/projects-and-services#wildcard-services) does.

`lokal down` removes the routes with the services, and `lokal list` shows them after the services, marked
`(lokal.toml)`. Routes exist only in `lokal.toml`: there's no command that adds one.

## Patterns

A key is a host pattern under the project's domain (a full name such as `{site}.seopages.lokal` works too). Each
label is a plain label or a capture, `{name}`, a whole label whose name is lowercase letters, digits and `_`,
starting with a letter.

- **The leftmost capture takes one or more labels.** `{site}` covers `acme.com.seopages.lokal` and takes
  `acme.com`.
- **Every other capture takes exactly one.** In `{page}.{tenant}`, `{tenant}` is one label and `{page}` the rest.
- **A plain label on the left pins the depth.** `preview.{slug}` covers `preview.x.myservice.lokal` and nothing
  deeper.
- **The domain itself is never covered.** `seopages.lokal` stays the `"@"` service's, if there is one.
- **A pattern needs a capture.** A key without one is an error that suggests one, and so are `pre-{x}`, `{x}y`,
  `{}`, `*` and a capture name used twice. A wildcard goes under `[services]` as `"*"`.

Only real labels are captured: letters, digits and `-`. A `Host` header with `/`, `%`, `?` or an empty label
matches no route, so nothing but a host name ever reaches the target's path.

## Targets

The value is the target URL, in one of two forms:

- **A service in the same file**, by its full name: `https://render.seopages.lokal/demosite/{site}`. lokal sends
  the request to that service's port directly. A wildcard service can't be a target.
- **A server lokal doesn't start**, on this Mac: `http://localhost:3000/preview/{slug}` (or `127.0.0.1`, or
  `[::1]`).

```toml
project = "myservice"

[services]
"@" = { run = "npm run dev", port = 3000 }

[routes]
"preview.{slug}" = "http://localhost:3000/preview/{slug}"   # preview.abc.myservice.lokal/x → /preview/abc/x
```

Each `{name}` in the path is replaced with what that capture took, and it must be one of the key's captures. A
query, a fragment, a placeholder in the host, or any other host or scheme is an error at the value's line and
column.

## What lokal changes in the request

- **The path.** `/` goes to the target path exactly (`/demosite/acme.com`, no trailing slash). Any other path is
  added to it: `/about` becomes `/demosite/acme.com/about`. The query stays as it is.
- **The `Host` header** is the target's: `render.seopages.lokal`, or that service's `localhost:<port>` if it has
  `host_header = "upstream"`, and `localhost:3000` for a `localhost` target. `X-Forwarded-Host` still has the
  name you opened, `acme.com.seopages.lokal`.
- **Redirects come back mapped.** If the target redirects into its own path, lokal takes the prefix off, so the
  browser stays on the site's name: `Location: /demosite/acme.com/new` arrives as `/new`, and
  `https://render.seopages.lokal/demosite/acme.com/new` as `https://acme.com.seopages.lokal/new`. Any other
  redirect passes untouched.
- WebSockets, server-sent events and streaming work as they do for any service. If the target isn't running, the
  error page names both: `acme.com.seopages.lokal → render.seopages.lokal/demosite/acme.com is not responding`.

## Pages only

lokal never rewrites a response body or a cookie's path. Every path under a routed name goes to the target under
the prefix, assets included: `/assets/app.js` on `acme.com.seopages.lokal` asks render for
`/demosite/acme.com/assets/app.js`. So the target has to serve the site's assets under the prefix too, or the page
loads them from another origin, such as `https://render.seopages.lokal`. Links the page writes should be relative
to the site's root (`/about`), as they are in production.

## Which one wins

1. A service with its own name: `render.seopages.lokal` is always the `render` service.
2. Otherwise, the most specific pattern, wildcard services and routes alike, comparing labels from the domain
   outward: a plain label beats a capture, and a longer pattern beats a shorter one. So `preview.{slug}` wins over
   `{site}`, as `"*.v1"` wins over `"*"`.

Two patterns that cover exactly the same names, such as `{site}` and `{tenant}`, or `{site}` and a `"*"` service,
are an error: lokal couldn't tell which one a request is for. In the file, the error names both lines. Against a
`"*"` service you added by hand, `lokal up` stops before it changes anything:

```text
error: route {site} in lokal.toml and service * cover the same names in seopages.lokal, so lokal couldn't tell which one a request is for
hint: remove the service (lokal service remove '*' -p seopages), or the route from lokal.toml
```

## Check a route

`lokal doctor` checks each route as `route.{site}.seopages.lokal`: whether its target service still exists, and
whether something listens where it forwards. Its `dns.` and `tls.` checks probe the pattern with each capture
replaced by `lokal-doctor`, such as `lokal-doctor.seopages.lokal`. A route whose target service you removed by hand isn't
served until the next `lokal up` adds the service back.

## Coming from Caddy

The same mappings in a Caddyfile, for comparison. For myservice, `{labels.2}` is the third label from the right
of `preview.abc.myservice.lokal`, `abc`:

```text
preview.*.myservice.lokal {
	rewrite * /preview/{labels.2}{uri}
	reverse_proxy localhost:3000
}
```

seopages needs a regular expression, because a customer's domain such as `acme.com` is two labels and Caddy's
`*` matches exactly one:

```text
# in a site block that receives every name under seopages.lokal
@site header_regexp site Host ^(.+)\.seopages\.lokal$
rewrite @site /demosite/{re.site.1}{uri}
reverse_proxy @site localhost:51234
```

lokal also does what these lines don't: it sends `/` as `/preview/abc` rather than `/preview/abc/`, sets the
target's `Host`, and maps redirects back to the name you opened.
