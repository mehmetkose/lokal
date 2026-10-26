# lokal

**Real HTTPS domains for your local dev servers, on macOS.**

`https://api.myapp.lokal` instead of `http://localhost:4000`, with a certificate that your browser, curl, Node
and Python trust. You set it up once. After that, adding a service takes milliseconds and never asks for a
password.

```console
$ lokal setup                        # once per Mac; asks for your password
$ lokal project create myapp         # claims myapp.lokal
$ lokal service add api 4000
✓ api.myapp.lokal → localhost:4000  (ready in 8ms)
  https://api.myapp.lokal
$ curl https://api.myapp.lokal       # works: no -k, no --cacert
```

When a command returns, the URL already works: the name resolves, the certificate exists and the route is
live. There's nothing to wait for and nothing to retry.

## Why lokal

- **Real names.** Every service gets its own hostname, so cookies, CORS, OAuth redirect URLs and subdomain
  routing behave the way they do in production. Services no longer share `localhost` cookies across ports.
- **Valid HTTPS.** Secure cookies, HTTP/2 and mixed-content rules work like they do in production, without
  `-k`, `--insecure` or clicking through browser warnings.
- **Fast and quiet.** Adding, changing or removing a service needs no sudo, no prompts and no restarts.
- **Nothing leaves your Mac.** Every listener is on loopback, and names are never announced on your network.
- **A narrowly scoped certificate authority.** lokal's CA can only sign `.lokal` and `.local` names, never
  public domains or IP addresses. Even a leaked key can't impersonate a real website.
- **It handles your whole dev stack.** WebSockets (Vite, Next.js and webpack hot reload), server-sent events,
  long polling, streaming uploads and HTTP/2 all pass through. `localhost` upstreams work whether your server
  listens on `127.0.0.1` or only on `::1`.
- **Scripts and coding agents can drive it.** Every command is safe to re-run, every list command has a stable
  `--json` output, and exit codes say what went wrong.

## Status

lokal is new (v0.1) and runs on **macOS only**: version 14 Sonoma or later, on Apple Silicon. It is developed
and tested on macOS 27 with Apple Silicon. Intel Macs and older macOS versions should work but haven't been
tested yet, so reports are welcome.

The code is structured so that Linux and Windows support can be added later ([`docs/PORTING.md`](docs/PORTING.md)).

## Install

### Homebrew

```console
$ brew install mehmetkose/tap/lokal
$ lokal setup
```

After `brew upgrade lokal`, run `lokal upgrade` so the background service switches to the new version. Before
`brew uninstall lokal`, run [`lokal uninstall`](#uninstall).

### Prebuilt binary

Each [release](https://github.com/mehmetkose/lokal/releases) has a tarball for Apple Silicon
(`aarch64-apple-darwin`) and one for Intel Macs (`x86_64-apple-darwin`), with SHA-256 checksums.

```console
$ v=0.3.0; t=aarch64-apple-darwin        # t=x86_64-apple-darwin on an Intel Mac
$ curl -fLO "https://github.com/mehmetkose/lokal/releases/download/v$v/lokal-$v-$t.tar.gz"
$ curl -fLO "https://github.com/mehmetkose/lokal/releases/download/v$v/lokal-$v-$t.tar.gz.sha256"
$ shasum -a 256 -c "lokal-$v-$t.tar.gz.sha256"
$ tar -xzf "lokal-$v-$t.tar.gz"
$ mkdir -p ~/.local/bin && install -m 755 "lokal-$v-$t/lokal" ~/.local/bin/lokal
$ lokal setup
```

The binaries are ad-hoc signed but not notarized by Apple. A tarball downloaded with `curl` runs as it is. If
you download it in a browser instead, macOS quarantines it and Gatekeeper blocks the binary, so run
`xattr -d com.apple.quarantine lokal-<version>-<target>.tar.gz` before you extract it. After you replace the
binary with a newer one, run `lokal upgrade`.

### From source

You need [Rust](https://rustup.rs) 1.89 or later and the Xcode Command Line Tools (`xcode-select --install`).

```console
$ git clone https://github.com/mehmetkose/lokal
$ cd lokal
$ make install        # builds and installs the CLI to ~/.local/bin (BINDIR=… to change)
$ lokal setup
```

After you pull changes, run `make install` again. Once lokal is set up, it also switches the background
service to the new build (`lokal upgrade`). `make uninstall` removes only the CLI. To remove everything, use
[`lokal uninstall`](#uninstall).

## Setup

`lokal setup` shows its plan and asks before it changes anything. On a fresh Mac it:

1. **Creates lokal's certificate authority** in `~/Library/Application Support/lokal/ca/`, as you. It can only
   sign names under `.lokal` and `.local`, and its private key never leaves that folder.
2. **Trusts it in the macOS System keychain**, for HTTPS only, so browsers and macOS tools accept lokal's
   certificates.
3. **Sends `.lokal` lookups to lokal** by installing `/etc/resolver/lokal`. This file tells macOS to ask lokal's
   built-in DNS server (`127.0.0.1:53535`) about `*.lokal` names.
4. **Installs the background service**, a LaunchAgent (`dev.lokal.daemon`) that runs as you. launchd holds ports
   443 and 80 on `127.0.0.1` and `::1` for it, so it never needs root, and nothing is reachable from your network.
5. **Runs a self-test.** It publishes a throwaway name, resolves it, and fetches it over HTTPS verified by macOS.

Steps 2 and 3 are the only ones that need your password. Setup prints these commands before it runs them, and
they are the only commands lokal ever runs with sudo:

```console
sudo /usr/bin/security add-trusted-cert -d -r trustRoot -p ssl -k /Library/Keychains/System.keychain ~/Library/Application\ Support/lokal/ca/root.pem
sudo /bin/mkdir -p /etc/resolver
sudo /usr/bin/install -m 0644 ~/Library/Application\ Support/lokal/resolver/lokal /etc/resolver/lokal
```

You type your password once in Terminal for sudo. macOS then shows its own dialog and asks for it again,
because it confirms every change to certificate trust settings separately.

- `lokal setup --dry-run` prints the plan and the exact commands without changing anything.
- Running `lokal setup` again is the repair path. It fixes whatever is missing, and asks for your password only
  if a privileged step is missing.
- Run it as yourself, not with `sudo`. lokal refuses to run setup as root.

## Usage

```console
$ lokal project create shop                    # https://shop.lokal; becomes the current project
$ lokal service add @ 3000                     # the apex itself: https://shop.lokal → localhost:3000
$ lokal service add api 4000                   # https://api.shop.lokal
$ lokal service add v1.api 4001                # nested names work too
$ lokal service add db-ui 8080 --host 127.0.0.1
$ lokal run web -- npm run dev                 # picks a port, routes https://web.shop.lokal, starts the server
$ lokal list                                   # every project, service and URL
$ lokal open api                               # open it in your browser
$ lokal service update api --port 4002
$ lokal service remove api
$ lokal project create legacy --domain legacy.local   # .local works too (see below)
```

A **project** owns one domain (`shop.lokal`), and each **service** is a subdomain routed to a port on your Mac.
`service`, `run`, `project show` and `open` use the current project. Pass `-p <project>` to pick another one.

`lokal run` skips choosing a port. It picks a free one (or reuses the service's), routes the service to it, and
starts your dev server with `PORT=<port>` and the terminal attached. For servers that ignore `PORT`, `{port}` in
any argument becomes the port: `lokal run web -- vite --port {port}`. lokal prints
`✓ ready: https://web.shop.lokal` on stderr once the server accepts connections, and exits with the server's exit
code.

| Command | What it does |
|---|---|
| `lokal setup` | One-time onboarding (see [Setup](#setup)) |
| `lokal doctor` | Checks every layer and explains how to fix what's broken |
| `lokal uninstall [--purge]` | Undoes setup (see [Uninstall](#uninstall)) |
| `lokal project create\|list\|show\|use\|remove` | Manage projects (`p` for short) |
| `lokal service add\|list\|update\|remove` | Manage services (`s` for short) |
| `lokal run <service> -- <command>` | Pick a free port, route the service to it and start your dev server |
| `lokal up [service]...` | Route every service in the repo's `lokal.toml` and start their dev servers (see below) |
| `lokal down` | Remove the services that `lokal up` added |
| `lokal list` | Every project with its services and URLs |
| `lokal start\|stop\|restart` | Control the background service |
| `lokal status` | Whether the service is running, its listeners and its published names |
| `lokal logs [service] [-f] [-n N]` | Show the background service's log, or the output of a service that `lokal up` started |
| `lokal upgrade` | Switch the background service to this `lokal` binary |
| `lokal env` | Shell exports so Node, Python and curl trust lokal (see below) |
| `lokal ca path` | Path of the CA certificate |
| `lokal open <service>` | Open a service in your default browser |

`lokal help <command>` shows details and examples. `list`, `status`, `doctor`, `project list`, `project show`
and `service list` take `--json`. That output is a stable contract (`"schema": 1`).

## A whole repo: `lokal.toml` and `lokal up`

Commit a `lokal.toml` at the repo root, and `lokal up` gets the whole stack running behind its domains:

```toml
project = "shop"                 # optional; default: the directory name

[services]
"@"  = { run = "npm start", port = 3000 }                    # https://shop.lokal, fixed port
api  = { run = "cargo run -p api", cwd = "services/api" }    # lokal picks a free port
web  = { run = "vite --port {port}", host_header = "upstream" }
docs = "mkdocs serve -a 127.0.0.1:{port}"                    # short form: the string is the command
```

```console
$ lokal up
✓ https://shop.lokal → localhost:3000  npm start
✓ https://api.shop.lokal → localhost:51234  cargo run -p api
✓ https://web.shop.lokal → localhost:51240  vite --port 51240
✓ https://docs.shop.lokal → localhost:51251  mkdocs serve -a 127.0.0.1:51251
  routes ready in 41ms
@    | > shop@1.0.0 start
api  |    Compiling api v0.1.0 (/Users/me/shop/services/api)
web  |   VITE v5.4.10  ready in 312 ms
  … the dev servers' own output …
✓ ready: 4/4 services accepting connections (2.8s)
^C
stopping 4 services… done. Routes kept; 'lokal down' removes them.
```

- `lokal up` finds `lokal.toml` in the current directory or a parent, and routes every service in it at once, so
  every URL works before any command starts. A service without `port` keeps the port it got last time.
- It then starts each `run` with `/bin/sh` in the service's `cwd`, with the same `PORT`, `{port}`, `LOKAL_URL` and
  `LOKAL_URL_<SUB>` that `lokal run` gives, and prefixes each line of output with the service's name.
- `✓ ready: N/N services accepting connections` is the line scripts and agents can wait for.
- Ctrl-C stops every service and everything it started, and keeps the routes. `lokal down` removes them.
- `lokal up api web` routes every service and starts only those two. One `lokal up` runs a project at a time.
- A server that listens on some other port gets a `hint:` naming it and the fix (`port = …` in `lokal.toml`, or
  `{port}` in its `run`). One that finds the port lokal chose already taken (`EADDRINUSE`) is started again on a
  new port, once. If the port it says is in use isn't its own, such as a debugger's, a `hint:` says so instead.
- Each service's output also goes to `~/Library/Logs/lokal/up/<project>/<service>.log`, each line after a
  timestamp, with lokal's own lines about it (started, exited, stopping) marked `lokal:`. `lokal logs api -n 50`
  shows the last lines from any terminal, also after `lokal down`, and `-f` follows them. Each `lokal up` starts
  the log afresh, and it keeps at most about 10 MB.

A `lokal.toml` runs commands, and the repo may be someone else's. So the first `lokal up` lists every command and
asks `run these commands? [y/N]`, and it asks again whenever the file changes. Without a terminal to ask on, it
changes nothing and exits 1. `--yes` runs the commands without asking and remembers nothing, for agents and CI in
your own repos. The full rules are in [SPEC §4.11](docs/SPEC.md#411-lokaltoml-lokal-up-and-lokal-down).

## Node, Python, curl and friends: `lokal env`

Browsers and most macOS tools use the keychain, so they trust lokal after setup. Node, Python and some other
tools ship their own certificate bundles instead. Point them at lokal's CA:

```console
$ eval "$(lokal env)"                 # zsh, bash
$ lokal env --shell fish | source     # fish
```

This sets `NODE_EXTRA_CA_CERTS` to lokal's CA, and `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` and `CURL_CA_BUNDLE` to
a bundle that holds the macOS system roots plus lokal's CA, so public HTTPS sites keep working. To make it
permanent, add the line to your shell profile.

## Vite and other dev servers that check the Host header

Vite (and some other dev servers) reject requests whose `Host` header they don't recognise. You have two
options. Allow your lokal names in the dev server:

```js
// vite.config.js
export default { server: { allowedHosts: ['.myapp.lokal'] } }
```

Or have lokal send `Host: localhost:<port>` upstream instead of the original host:

```console
$ lokal service add web 5173 --host-header upstream
$ lokal run web --host-header upstream -- vite --port {port}   # or let lokal pick the port
```

The original host is still available in `X-Forwarded-Host`. lokal also sets `X-Forwarded-Proto: https` and
`X-Forwarded-Port: 443`, and appends the client to `X-Forwarded-For`.

## Using lokal with coding agents

lokal's root help (`lokal --help`) includes a short section written for coding agents, so an agent can learn the
workflow from the binary alone. The commands are safe to repeat, and `lokal list --json` and
`lokal status --json` give machine-readable state. An agent should never run `lokal setup` itself, because setup
needs your password. If `lokal status` says lokal isn't set up, the agent should ask you to run it.

To tell an agent about lokal in your own project, add something like this to its `AGENTS.md` or `CLAUDE.md`:

```markdown
## Local HTTPS URLs (lokal)
- This repo's dev servers run behind lokal: https://web.myapp.lokal → localhost:3000,
  https://api.myapp.lokal → localhost:4000.
- Set up routes with `lokal project create myapp` and `lokal service add <sub> <port>` (safe to re-run).
- Or start a dev server on a port lokal picks: `lokal run <sub> -- <dev command>`, then wait for
  `✓ ready:` on stderr.
- Or, with a `lokal.toml` in the repo: run `lokal up --yes` in the background and wait for `✓ ready:` on stdout.
  Stop it with Ctrl-C or SIGTERM; `lokal down` removes its routes.
- One `lokal up` runs per project. If it says `lokal up is already running`, use that stack and don't stop it;
  `lokal logs <sub>` shows its output.
- When a .lokal URL fails, `lokal logs <sub> -n 50` shows that service's recent output (for services that
  `lokal up` started).
- When a URL fails, run `lokal doctor`. Never run `lokal setup` or sudo; ask the user.
```

## `.lokal` or `.local`?

New projects get `.lokal` names. `.local` names keep working through `--domain <name>.local`.

| | `.lokal` (default) | `.local` |
|---|---|---|
| Resolved by | lokal's own DNS server, through `/etc/resolver/lokal` | macOS's mDNSResponder (Bonjour), as local-only records |
| New names | Work at once, even if you looked the name up before adding it | Work at once |
| Unknown names | Resolve to loopback, but get no certificate (and a 404 over plain HTTP) | Take about 5 seconds to fail (the standard multicast timeout) |
| Reserved TLD | No (see the [security model](docs/SECURITY-MODEL.md#the-lokal-tld)) | Yes, for multicast DNS (RFC 6762) |

Either way, names never leave your Mac.

## How it works

- **Names.** The background service answers every `*.lokal` lookup with `127.0.0.1` and `::1`, with a zero
  TTL, so nothing is cached. `.local` names are registered with mDNSResponder on the local-only interface,
  which keeps them off your network.
- **Certificates.** Each name gets its own certificate, issued in memory by lokal's CA when you add the
  service. Nothing touches the keychain after setup.
- **Ports.** launchd owns the 443 and 80 sockets and hands them to the background service, which runs as you.
  Connections that arrive while it restarts wait in launchd's queue instead of failing.
- **Routing.** The proxy routes by `Host` and SNI to `localhost:<port>`, streaming bodies in both directions.

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) walks through the design and the code.

## Security

Installing a certificate authority is a big ask, so here is what lokal does to limit it:

- The CA is **name-constrained** to `.lokal` and `.local`, and excludes every IP address. Certificate verifiers
  reject anything it signs for other names, so even its private key can't impersonate a real website, a company
  intranet host or your router.
- The private key stays in your user folder (`ca/root-key.pem`, readable only by you). It is never copied,
  logged or sent anywhere, and lokal issues certificates only for names you configured.
- Every listener is on loopback. After setup nothing runs as root. lokal asks for your password again only in
  `lokal uninstall`, or in `lokal setup` if it has to repair a privileged step.
- The only network traffic lokal sends off your Mac is one DNS query in each `lokal doctor` run, which asks the
  public DNS root whether `.lokal` exists as a real top-level domain.

[`docs/SECURITY-MODEL.md`](docs/SECURITY-MODEL.md) covers what an attacker could and couldn't do, and every
file and setting lokal changes. To report a vulnerability, see [`SECURITY.md`](SECURITY.md).

## Troubleshooting

Start with `lokal doctor`. It checks every layer (setup, the CA and its trust, the background service and its
build, the listeners, a self-test, and each service's DNS, TLS and upstream) and says how to fix what's broken.
`lokal logs -f` follows the service log (`~/Library/Logs/lokal/daemon.log`). When a .lokal URL fails,
`lokal logs <sub> -n 50` shows that service's recent output (for services that `lokal up` started).

[`docs/TROUBLESHOOTING.md`](docs/TROUBLESHOOTING.md) covers common problems: certificate errors in Node or
Python, `502` pages, names that don't resolve, port 443 already in use, and more.

## Uninstall

```console
$ lokal uninstall            # removes the service, the resolver file and the CA's trust (asks for your password)
$ lokal uninstall --purge    # also deletes your projects, the CA files and the logs
$ make uninstall             # then remove the CLI itself, if you installed it with make
```

A plain uninstall keeps `state.toml` and the CA files, so running `lokal setup` later restores everything as it
was. Use `--purge` if you're done with lokal for good.

## FAQ

**How is this different from mkcert?** mkcert creates locally trusted certificates. You still have to configure
DNS (or `/etc/hosts`) and a server or proxy for each name. lokal does names, certificates and routing together,
and its CA is name-constrained, so it can only sign `.lokal` and `.local` names.

**Does it work with Docker?** Yes, for ports you publish on your Mac. With `docker run -p 8080:80 …`, run
`lokal service add app 8080`. Containers can't resolve lokal's names themselves.

**Can I open my dev URLs on my phone or another computer?** Not yet. lokal listens only on loopback by design.
A LAN mode is on the [roadmap](docs/ROADMAP.md).

**Does it edit `/etc/hosts`?** No. `lokal doctor` warns if `/etc/hosts` has an entry that conflicts with a lokal
name.

**Why only `.lokal` and `.local`, and not `.test` or `.internal`?** The CA is trusted system-wide, so it is
constrained to exactly the TLDs lokal serves. Adding `.test` or `.internal` would let a leaked key sign names that
companies use for real internal hosts.

**Why does setup ask for my password twice?** Once is sudo in Terminal. The second time is a macOS dialog,
because macOS asks separately before it changes certificate trust settings.

## Contributing

Contributions are welcome. [`CONTRIBUTING.md`](CONTRIBUTING.md) explains how to build, test and send a change,
with or without a coding agent. The full design is in [`docs/SPEC.md`](docs/SPEC.md), and [`docs/`](docs/README.md)
indexes the rest.

lokal v0.1 was built by a coding agent working from that spec, with human sign-off for every privileged step.
[`docs/history/`](docs/history/README.md) has the build log, the decisions and the measurements made on macOS.

## Acknowledgements

The launchd socket-activation approach, where launchd binds the privileged ports for an unprivileged agent,
follows [puma-dev](https://github.com/puma/puma-dev).

## License

Licensed under the [Apache License, Version 2.0](LICENSE). Unless you explicitly state otherwise, any
contribution you intentionally submit for inclusion in lokal, as defined in the Apache-2.0 license, is licensed
under the same terms, without any additional terms or conditions.
