# Roadmap

Ideas planned for after v0.1. **Nothing here is built yet,** except §3 (`lokal run`, released in 0.2.0) and
most of §1 (`lokal up`, built but not yet released), and the details may change. Each item needs an update to
[`SPEC.md`](SPEC.md) before work starts. Items that change the security requirements (SPEC §16) also need
explicit sign-off.

Feedback is welcome. If one of these would help you, or you'd design it differently, open an issue.

## 1. `lokal.toml` and `lokal up`: one command starts a repo's whole stack (built)

**Built, not yet released.** A repo commits a `lokal.toml`, and `lokal up` routes every service in it in one
reload, then starts each service's `run` in its own process group with `PORT`, `{port}` and `LOKAL_URL_<SUB>` for
every service, prefixes each line of output with the service's name, and prints
`✓ ready: N/N services accepting connections`. Each service's output also goes to a log file that
`lokal logs <service>` reads. Ctrl-C stops every service and keeps the routes, and `lokal down` removes them. A
person approves a file's commands before they first run, and again after it changes. The rules are in
[SPEC §4.11](SPEC.md#411-lokaltoml-lokal-up-and-lokal-down). Still to come: `lokal run`'s wrong-port hint, and a
retry when a picked port turns out to be taken.

The original plan:

**Why.** A teammate or a coding agent should get a repo's dev servers running behind their domains with one command
and no port archaeology. The repo commits one file, and `lokal up` routes every service and starts its dev server.

```toml
# lokal.toml at the repo root
project = "shop"                 # optional; default: the directory name

[services]
"@"  = { run = "npm start", port = 3000 }                    # https://shop.lokal, fixed port
api  = { run = "cargo run -p api", cwd = "services/api" }    # lokal picks a free port
web  = { run = "vite --port {port}", host_header = "upstream" }
docs = "mkdocs serve -a 127.0.0.1:{port}"                    # short form: the string is the command
```

**Sketch.**
- Every service has a `run` command. Servers that lokal doesn't start (a container, something started by hand)
  stay with `lokal service add`.
- `lokal up` creates the project, picks ports, and routes every service in one reload, so every URL is live before
  any command starts. It then runs the commands in the foreground with prefixed, interleaved output. Ctrl-C stops
  them all.
- Each command gets `PORT`, `{port}` substitution, its own URL in `LOKAL_URL`, and every other service's URL
  (`LOKAL_URL_API=https://api.shop.lokal`), so services can call each other by name.
- Each service's output also goes to a log file that `lokal logs <service>` reads.
- **Trust prompt.** `lokal up` runs commands from a file in someone else's repo. So the first run, and any run after
  the file changes, lists the commands and asks before running them. `--yes` skips the prompt.
- `lokal down` removes the routes that `lokal up` added.

## 2. LAN mode: open your dev URLs on a phone

**Why.** Test `https://api.shop.local` on a real phone on the same Wi-Fi, with a valid certificate.

**Sketch.**
- Opt-in per project (`lokal lan on -p shop`), off by default, and clearly shown in `status` and `doctor`.
- Publish the opted-in names on the real network interfaces with the Mac's LAN addresses, and accept connections
  on the LAN address only for those names.
- `lokal lan profile` writes a configuration profile with the CA for iOS (Android needs a user CA). The CA's name
  constraints still limit what it can sign to `.lokal` and `.local`.
- This changes the "listeners are loopback-only" rule, so it needs a security review first. Open questions
  include pairing or client allowlists, and warnings on untrusted networks.

## 3. `lokal run <service> -- <command>`: no port numbers at all (built)

**Released in 0.2.0.** `lokal run web -- npm run dev` picks a free port (or reuses the service's), routes
`https://web.<project>.lokal` to it, and starts the command on it with `PORT=<port>`, a `{port}` placeholder for
servers that ignore `PORT`, and `LOKAL_URL_<SUB>` for every service in the project. It prints `✓ ready: https://…`
once the server accepts connections, and exits with the command's exit code. The rules are in
[SPEC §4.10](SPEC.md#410-lokal-run). Its port picking, environment, signal handling and tests are written to be
reused by `lokal up` (§1).

## 4. Error trips: show the coding agent where an error started

**Why.** When several services run together, the service that shows an error is rarely the one that caused it.
lokal sits in the request path and, with `lokal up`, owns every process's output. So it can show the whole trip of
a failing request and name the service where it started.

```text
error trip  req a1f3  (412 ms)
  console.shop.lokal  GET  /billing          → 500   shows "something went wrong"
  └─ api.shop.lokal   GET  /v1/invoices      → 500   "upstream auth failed"
     └─ auth.shop.lokal  POST /introspect    → 401   ← origin
        auth | token audience mismatch: expected "api", got "console"   src/verify.ts:88
```

**Principles.**
- lokal captures and correlates. The coding agent diagnoses. There's no model inside lokal.
- Everything stays on the machine: no account, no collector, no required instrumentation.
- Built in small steps, each useful on its own: per-service logs (§1), recorded proxy hops with W3C
  `traceparent`, a `lokal events --errors --json` command for agents, request replay with credentials redacted,
  and later an OpenTelemetry receiver.
- Recording requests touches credentials and customer data, so this needs a security review first: local-only,
  0600 files, short retention, and `Authorization`/`Cookie` redacted by default.

## 5. `lokal share`: public URLs for a dev stack, through Cloudflare Tunnel

**Why.** Show a running stack to a reviewer, QA, a client or a webhook provider without deploying it.

**Sketch.**
- You bring your own Cloudflare account. `lokal share --setup` connects it and picks a zone and base name
  (`dev.example.com`).
- `lokal share api web` runs `cloudflared` with one rule per shared service, pointing at lokal's proxy with TLS
  verification on. Ctrl-C stops sharing and removes the tunnel and its hostnames.
- Sharing is always explicit, per command. Nothing is public by default, and lokal's own listeners stay
  loopback-only, because `cloudflared` dials out.
- Credentials are stored in the macOS keychain. Public exposure needs a security review first.

## Smaller items

- **Upstream HTTPS:** proxy to dev servers that only speak HTTPS.
- **`service enable` / `service disable`:** keep a service configured but stop serving it.
- **Shell completions** for zsh, bash and fish.
- **Linux support:** the platform seam is ready. [`PORTING.md`](PORTING.md) collects what each OS needs.
