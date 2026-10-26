# Testing

lokal is tested in layers. The lower layers run anywhere and change nothing on your Mac. The live test proves
the parts that only a real install can: that macOS resolves the names, and that browsers and tools trust the
certificates.

| Layer | Command | Needs | Changes your system? |
|---|---|---|---|
| Format and lints | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` | Rust | no |
| Unit tests | `cargo test --workspace` | Rust | no |
| Sandbox integration tests | (part of `cargo test`) | macOS for the mDNS test | no: `LOKAL_HOME` temp dir, high ports, throwaway CA |
| Platform seam | `crates/lokal/tests/seam.rs` (part of `cargo test`) | nothing | no |
| Other platforms compile | `cargo check --workspace --target x86_64-unknown-linux-gnu` | that rustup target | no |
| Live test | `scripts/live-test.sh [--full]` | lokal set up on this Mac, Google Chrome | swaps the background service for your build (`lokal upgrade`); adds and removes a test project |
| Clean room | `scripts/clean-check.sh` | after `lokal uninstall` | no (read-only) |
| Release packaging | `scripts/package.sh aarch64-apple-darwin` | macOS | no: writes the tarball to `dist/` |
| Homebrew formula style | CI job `formula`: renders `packaging/homebrew/lokal.rb.in` with dummy checksums into a `Formula/` folder and runs `ruby -c` and `brew style` on it, the style rules `brew audit --strict` applies at release | Homebrew | no |

## Unit and integration tests

- **Unit tests** sit next to the code. A few pin security-relevant details:
  - `ca::tests::ca_fields_match_spec` checks every CA field against SPEC §6.1 with `x509-parser`;
  - `ca::tests::ca_must_exclude_every_ip_address` checks the IP exclusion in the name constraints;
  - `cli::help::tests` checks that root help lists every command.
- **`crates/lokal/tests/cli.rs`** has one test per validation rule in SPEC §4.3, plus the `--json` shapes,
  20 parallel `service add` calls that must lose no writes, unknown `state.toml` fields surviving a rewrite
  (SPEC §5.2), and `service remove` with several names: all or nothing, a repeat counted once (SPEC §4.5). All
  run under `LOKAL_HOME`.
- **`crates/lokal/tests/proxy.rs`** runs the real daemon in a sandbox
  (`lokal daemon run --foreground --https-port <p> --http-port <q> --name-backend none`) with a sandbox CA. It
  covers HTTP/1.1 and h2, SSE timing, WebSockets, a 50 MB streamed POST, 502/404/308/421, `X-Forwarded-*`, both
  `host_header` modes, `::1`-only and `127.0.0.1`-only upstreams, unknown or missing SNI, and reload readiness,
  including `service remove` of two names returning only once neither is served.
- **`crates/lokal/tests/limits.rs`** runs the same sandbox daemon with small connection caps, short deadlines or
  a 48-file open-file limit (SPEC §9.3). It checks that every idle state (silent TCP, a partial request, quiet
  TLS with each ALPN, h2 with no stream, after a lokal-built page) is closed by its deadline while a slow
  proxied request and a quiet WebSocket are not; that a full cap of idle connections still admits a real
  request and `lokal status`; that new connections are refused only when every connection is busy; that the
  upstream cap answers `503`; and that idle sockets can't exhaust the daemon's file descriptors.
- **`crates/lokal/tests/run.rs`** covers `lokal run` (SPEC §4.10) against the same sandbox daemon. It checks that
  the URL works on the first request after `✓ ready:`; the command's environment (`PORT`, `HOST`, `LOKAL_URL`,
  `LOKAL_URL_<SUB>`); exit codes (the command's, 128 + N for a signal, 127, 126); reuse of an existing service
  and the usage errors for a different `--port` or `--host-header`; `--rm`; a busy port (exit 1, the owner named,
  nothing started); the wrong-port hint, which must name the port of a server the command's child started and
  not a listener elsewhere in lokal's process group (cargo test runs everything in one group), no sooner than the
  second lookup at 5s, and must stay silent for a command that opens a helper listener at once and its own port
  3.5s later; and the notes when no daemon runs. Two tests count the signals that
  reach the command with `sig-probe` (`crates/testkit/src/bin/sig-probe.rs`), which exits with 40 + the count:
  a Ctrl-C written to a pty that is lokal's controlling terminal must arrive exactly once (not relayed again),
  and with no terminal lokal must relay SIGINT and SIGTERM once. `sig-probe` also fails if it starts with a
  signal ignored, which catches a wrapper that set `SIG_IGN` before starting it. The test builds `echo-server`
  and `sig-probe` with `cargo build -p testkit` on first use, since `cargo test` builds only testkit's library
  for the `lokal` crate.
- **`crates/lokal/tests/up.rs`** covers `lokal.toml`, `lokal up` and `lokal down` (SPEC §4.11). Each test makes a
  temp repo and runs lokal in it under `LOKAL_HOME`: the file's errors (exit 2, naming `lokal.toml:<line>:<col>`)
  and unknown-key warnings, finding the file from a subdirectory, a nested file as its own project, the derived
  project name, a second `up` leaving `state.toml` byte-identical, hand-added services surviving `up` and `down`,
  adopting a hand-added service (or a `lokal run` route) whose route is the file's, clashes for routes that
  differ (naming the difference, with one `service remove` command as the hint), domains, busy fixed and kept
  ports, and the other commands on managed services. Against the sandbox daemon, every URL answers with the 502
  page as soon as `up` returns and fails the TLS handshake as soon as `down` returns, and `lokal run` on a managed
  service keeps its marker. The parsing rules and the port plan (with a fake
  probe and port picker) have unit tests in `project_file.rs` and `cli/up.rs`.
  - **The trust check** (SPEC §16.11). With no terminal (stdin closed) and no `--yes`, `up` exits 1, prints the
    path and every command, leaves `state.toml` byte-identical, writes no approval, and neither stream contains
    `--yes`. With `--yes` (or `-y`) it prints the listing and its note, routes, and leaves `<LOKAL_HOME>/trusted/`
    empty. Through a pty (lokal leads a session with it as its controlling terminal, the helper in
    `tests/common/mod.rs` that `run.rs` uses too), `n`, an empty answer and Ctrl-D exit 1 with nothing routed and
    no record, and `y` exits 0 and writes the record: 0600 in a 0700 directory, named by the SHA-256 of the file's
    real path, holding the SHA-256 of its bytes and a newline. After that, a run with no terminal needs nothing;
    after a comment is appended, it exits 1 saying the file changed, and `--yes` leaves the old record alone. A
    symlinked repo path uses the same approval, and a copy of the file elsewhere asks again. A repo whose
    `lokal.toml` is a symlink to another repo's is that repo's file: its approval holds with no terminal, the
    project is the real file's (the derived name), a `cwd` resolves under the real file's directory, and `down`
    there removes that project's services. The tests that aren't about the trust check pass `--yes`. Unit tests
    cover the decision table, the record (a match, another digest, missing, unreadable), the digest `load` takes
    and the real path everything comes from (through a symlinked directory, a symlinked file and `..`, for `load`
    and `load_project_name`), and the listing, including that no control or invisible character in the file
    reaches the terminal.
  - **Running the commands.** With two echo-servers against the sandbox daemon, the route lines come before
    `✓ ready: 2/2 services accepting connections`, and each URL returns its marker on the first request. A
    service's stdout line shows on stdout as `a  | LISTENING 127.0.0.1:<port>` (padded to the longest name) and a
    stderr line (`echo oops >&2`) on stderr, with lokal's own lines and the §4.5 note on stdout and
    `✓ listening:` when no daemon took the routes. The shell gets `{port}`, `PORT`, `HOST`, `LOKAL_URL`,
    `LOKAL_URL_<SUB>`, the file's `env` and its `cwd`. Ctrl-C written to a pty (lokal's controlling terminal) and
    SIGTERM with no terminal (`setsid`) each reach two `sig-probe` services exactly once (`signals: 1`), lokal
    exits 130 or 143, and both process groups are gone. A `sleep` started with `&`, which ignores SIGINT, keeps
    lokal waiting until the SIGTERM at 5s; with `trap '' INT TERM`, a second SIGINT ends lokal within 1s. A
    service's `exited` line comes after its last output, also when that output fills more than a pipe's buffer.
    `lokal up a` starts only `a` and still routes `b`. While one `up` runs, a second `up` and `down` exit 1, name
    its pid and change nothing, and the lock file (0600, in a 0700 folder) doesn't block the next `up` once
    lokal exits. Commands that all `exit 0` give exit 0, `exit 3` gives `b exited 3` and exit 1, and an unknown
    command puts `x | … command not found` on stderr and `x exited 127`. A 40000-character line comes out as
    16 KiB pieces, and a closed stdout stops the services (exit 141) instead of killing lokal. Every
    long-running command a test starts has a unique tag in its command line, and the test's guard stops lokal
    and then kills whatever still carries the tag, so a failed test leaves nothing running. Unit tests cover what
    each command gets (`starts_of`), line splitting at a character boundary, and, in `process.rs`, that
    `spawn_shell`'s shell leads its own process group, which outlives it while a background job is left.
  - **Servers that detach** (SPEC §4.11, "A server that detaches"). The testkit echo-server's `--detach` starts
    the server in a new session (`setsid`) with its streams on `/dev/null`, prints `DETACHED <pid>` and exits 0:
    once the server listens and 300ms later (`fast`, a fixed port: the ready probe sees the port first), or at
    once, with the server binding 500ms later (`slow`, a picked port). Each exit line names the server's pid
    (`fast exited 0, and left localhost:<port> to echo-server (pid <pid>), which runs outside lokal up: …`), the
    hint with the link to the frameworks guide follows it, both are the log's last lines, and the one ready line
    reads `✓ listening: 2/2 services on localhost; detached: fast, slow`. lokal exits 0 and ends with
    `2 servers keep running outside lokal up: fast (echo-server, pid <pid>), slow (echo-server, pid <pid>)`, and
    `detached.json` is 0600. The next `up`, and `up slow`, exit 1 with `… already in use by echo-serv (pid <pid>),
    which fast left running at the last lokal up; not starting fast` and the hint `stop it: kill <pid>. …`. The
    test then kills both servers by the pids lokal named (each checked for the test's tag first), and an `up`
    that starts both services again with `true` removes the record. A `run` of `true` gets a plain
    `a exited 0`, `; not ready: a (exited 0)`, no record, and the ready line within 2.5s of the route lines. An
    echo-server started with `& exit 0` holds the port in the service's own group: `a exited 0` comes before
    `✓ listening: 1/1 services on localhost`, nothing says detached, and Ctrl-C stops the job with its group at
    the SIGTERM at 5s. Unit tests (`cli/up.rs`) cover what an exit 0 left (an owner in the group; listeners
    outside it, with only-bound sockets left out; nothing), the exit line for one server and several (names
    escaped), the hint and its link's heading, the ready line's counts and names, the last line, the record
    (replacing a service's entries, forgetting the started ones, a field it doesn't know), and the busy error,
    which names a server as left only when its pid, full name and port match the record and it listens.
  - **Restarting a service** (SPEC §4.11, "Restarting a service"). With two echo-servers against the sandbox
    daemon, `lokal restart a` exits 0 with one line, `✓ restarted https://a.p.lokal (…)`, and a's URL returns its
    marker on the first request after it returns; a's echo-server is a new process and b's the same one, a's log
    has `lokal: restarting (lokal restart)`, `lokal: stopping (SIGINT)`, `lokal: exited 130` and a second
    `lokal: started on port …`, `up` printed `restarting a` and `a ready (…)` and no `a exited`, keeps running,
    and exits 130 on SIGINT. `up.sock` is 0600 in the 0700 folder while `up` runs, and gone once it exits. A run
    that fails when a flag file exists makes `lokal restart a` exit 1 with `error: a exited 1 before it was
    ready`, that run's three log lines (not the first run's), and `hint: lokal logs a -n 50`; without the file,
    the next restart starts the exited service again. With no `up` running, `lokal restart a` exits 1 with
    `no lokal up is running for p` and `hint: start it: lokal up`; an unknown SUB, alone or with a known one, and
    a malformed one exit 2 and restart nothing; plain `lokal restart` is still the background service's (refused
    under `LOKAL_HOME`), and `-p` without SUB exits 2. After `lokal up a`, `lokal restart bee` prints `starting
    bee` in `up`, starts it with a fresh log that begins `lokal: starting (lokal restart)`, and `up` then stops
    2 services. After `lokal.toml` changes, `lokal restart a` prints the note and runs the command `up` started
    with; with the same bytes back, no note. A service that ignores SIGINT restarts at the SIGTERM at 5s, and two
    `lokal restart a` started together both return then, from one new run; while `up` stops, `lokal restart b`
    exits 1 with `error: lokal up is stopping`. A service whose server detached answers with the busy error that
    names it as left by the service, and once the test has killed it, the restart prints the detached hint and
    `✓ restarted`. The lock test also checks that a second `up` gets the restart hint, naming `a` when it was
    `up a`. Unit tests (`cli/up.rs`, `ipc/up.rs`) cover the wire format of the request and every event, the
    restart's busy hint and its detached wording, the refused `up`'s hint, and the last run's 20 lines that
    `lokal restart` shows.
  - **Logs** (SPEC §4.11, "Logs"). A service's `<sub>.log` (0600; `v1.api.log` for `v1.api`) has its stdout and
    stderr lines and lokal's `lokal: started on port <port>: …` and `lokal: exited 3`, each line starting with
    `^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6}Z `; `lokal logs a -n 3` prints the file's last 3 lines, and the
    log still reads after `down --remove-project`, with SUB normalised against the file's apex
    (`a.shop.local`). `lokal logs a -f`, started while `up` runs, shows a line the service prints after it
    started, and the log ends with `lokal: stopping (SIGINT)` and `lokal: exited 130`. The next `up` truncates
    the log and removes an older `a.log.1`, while a service it doesn't start keeps its log. A service that
    prints 6 MB leaves `a.log.1` (full to within a line of 5 MB) and `a.log`, and `-n` reads back across both.
    Plain `lokal logs` still tails `daemon.log`, also in a repo. An unknown SUB exits 2, a service from
    `service add` or `lokal run` exits 1 with the no-log message, and `-p` without a SUB exits 2. In the repo
    and below it, `lokal logs a` finds the file's project while another project is current; outside it, §4.4
    applies, and `-p` wins over both. Unit tests (`cli/up_log.rs`, `util.rs`) cover the timestamp format,
    truncation, rotation at a small cap with nothing lost, a write error that warns once and stops the log,
    reading the last lines across `.log.1` and `.log`, and the apex `load_name` reads from a file with errors.
  - **The wrong-port hint and the retry** (SPEC §4.11, "Ports" and "The wrong-port hint"). A service whose `run`
    starts an echo-server on a port of its own (no `{port}`, no `port`) gets exactly one `hint:` on stdout, no
    sooner than 5s after the start (two lookups must agree), naming `localhost:<P>`, `port = <P>` and `{port}`,
    and its log has it as `lokal: hint: …`; one whose `run` has `{port}` (in a shell comment) gets only the
    `port =` part, and a `sleep` gets none. One whose `run` starts a helper echo-server at once and its own on
    `{port}` 3.5s later, between the lookups, gets no hint before its ready line, nor in its log. In the retry
    tests the test holds the service's port with a socket bound but not listening, so neither the busy check nor
    the ready probe sees it, but lsof shows the test as its owner, outside the service's process group: the
    `run` waits for a `go` file until the test, having read the route line, holds the port. With the sandbox
    daemon, a `run` that prints `EADDRINUSE` and exits 1 the first time prints
    `a exited 1`, then `note: a's port <old> was taken; retrying on <new>`, then `✓ ready: 1/1`, and no hint;
    `list --json` has the new port, the URL returns the marker on the first request, and the log has
    `lokal: exited 1`, `lokal: port <old> was taken; retrying on <new>` and a second
    `lokal: started on port <new>: …`. A `run` that always fails that way (`Address already in use` on stderr) is
    retried once and exits 1 after a second `x exited 1`, followed by the port-in-use hint for its new port,
    which nothing holds; one with a fixed `port` is never retried, keeps its port, and gets the hint. A `run`
    that prints workerd's `bind(): Address already in use; port 9229` and exits 1 while its own port is free
    isn't retried, without `port` or with one: its port stays, and the hint (`… says a port is in use, but its
    own port <P> is free …`, with the link to the frameworks guide's debugger ports) comes after its exit line,
    before the ready line, and in its log as `lokal: hint: …`. A kept port that the test holds makes echo-server
    fail with its own `Address already in use`, and the service moves. A `sleep` that the first run leaves in its
    group, ignoring SIGINT, keeps lokal waiting after Ctrl-C until the SIGTERM at 5s, with `stopping 1 service…`.
    Unit tests (`cli/up.rs`, `cli/run.rs`, `runner.rs`, `platform/`) cover when two lookups agree, the list of at
    most three ports, the hint's three wordings (one port; several, none offered; a port the file or `--port`
    fixes), the match, the rule for an own port that's taken (nothing there; an owner in the service's group;
    another group listening or only bound; a connection with no owner lsof can see), the port-in-use hint and
    its link's heading in the frameworks guide, the parsing of `lsof -F pgcnT` (only unconnected sockets on
    the port), `port_owner`'s text (listeners only, as before), the retry's state change (only a managed service that
    still has the old port; the pick skips every port in state) and what the retried command gets.
- **`crates/lokal/tests/mdns.rs`** (macOS only) uses the real mDNSResponder backend. It checks that `getaddrinfo`
  sees a random `lt-<hex>.local` name as soon as `reload` returns, and that the name is gone as soon as removal
  returns. The names are local-only and invisible to your network.
- **`crates/lokal/tests/seam.rs`** fails if `cfg(target_os)` appears outside `src/platform/`, or if a platform
  trait method lacks a `PORTING(...)` note.

The `check` probes (`crates/testkit/src/bin/check.rs`) accept `--cacert <file> --connect <ip:port>` **only** for
these sandbox tests. The live test never passes them.

## The live test

`scripts/live-test.sh` builds the workspace in release mode and runs `crates/testkit/src/bin/live-test.rs`
against the lokal installed on this Mac. Arguments pass through:

- `--full` also runs L23, the performance targets in SPEC §14;
- `--lokal <path>` tests a different `lokal` binary (the default is `target/release/lokal`).

It prints one line per check, `PASS|FAIL|SKIP <id> <description> (<duration>) [detail]`, writes
`target/live-test-report.json`, and exits 1 on any FAIL. Saved reports from releases are in
[`live-test-reports/`](live-test-reports/).

Runner rules:

- The test project is `lt-<6 hex>` (written `<p>` below), under `.lokal`. A second project, `<p>-m.local`, covers
  the `.local` (mDNS) path in L04, L05, L06, L19 and L24. Upstream ports are found by binding to port 0.
- **Readiness checks don't poll.** The CLI promises that the URL works when the command returns, so the first
  attempt must succeed. Only the crash-recovery and stop/start checks poll, every 50ms up to their stated limit.
- Before it starts, the runner snapshots `lokal list --json` (and the current project). It always cleans up the
  test projects and upstreams, even on failure, and restores the current project.
- TLS is verified with `rustls-platform-verifier`, so the probes trust exactly what macOS trusts. Nothing passes
  `-k`, `--insecure` or `--ignore-certificate-errors`.

| ID | Check | Pass condition |
|---|---|---|
| L01 | Setup is idempotent without sudo | `LOKAL_FORBID_SUDO=1 lokal setup --yes` exits 0 and reports that everything is already done |
| L02 | Baseline doctor | `doctor --json` has no `fail` (warnings from the developer's own projects are fine), and `selftest` passes |
| L03 | Upstreams | echo-server A on `127.0.0.1:<a>`, B on `::1:<b>` **only**, D on `127.0.0.1:<d>`; port C stays closed |
| L04 | Create | `project create <p>`; `service add api <a>`; `service add web <b>`; `service add down <c>`; `service add @ <a>`, and the `.local` project with `api`. Each exits 0 and prints `ready in`. Re-running `project create` and an identical `service add` succeeds |
| L05 | Resolution, first try | right after L04: `check resolve` for every FQDN returns **both** 127.0.0.1 and ::1 on the first attempt, first lookup < 20ms, repeat < 5ms |
| L06 | HTTPS via system trust | `check tls api.<p>.lokal` passes. `curl -sS --fail https://api.<p>.lokal/` with **no** `-k` and no `--cacert` returns marker A. Same for the `.local` project |
| L07 | Real browser | Google Chrome headless with a fresh `--user-data-dir`: `--headless=new --dump-dom --virtual-time-budget=3000 https://web.<p>.lokal/`. Output contains marker B (which proves the `::1`-only upstream works through `localhost`) and no `ERR_` |
| L08 | macOS trust evaluation | `security verify-cert -c <leaf from openssl s_client -showcerts> -p ssl -s api.<p>.lokal -L` exits 0 |
| L09 | Apex | `https://<p>.lokal/` returns marker A |
| L10 | Redirect | `curl -sI http://api.<p>.lokal/x?y=1` → `308`, `Location: https://api.<p>.lokal/x?y=1` |
| L11 | WebSocket | `check ws wss://api.<p>.lokal/ws` passes |
| L12 | SSE | `check sse https://api.<p>.lokal/sse` receives 3 events, each within 300ms of being sent |
| L13 | Upstream down | `https://down.<p>.lokal/` → 502, and the body mentions `localhost:<c>` |
| L14 | Unknown names | TLS to unconfigured `nope.<p>.lokal` (forced with `curl --resolve nope.<p>.lokal:443:127.0.0.1`) fails the handshake. HTTP to it on port 80 returns 404 |
| L15 | Hot add | `service add new <d>`, then **immediately** `https://new.<p>.lokal/` returns marker D on the first try. The daemon PID is unchanged |
| L16 | Hot update | `service update api --port <d>`, then immediately `https://api.<p>.lokal/` returns marker D on the first try |
| L17 | Hot remove | `service remove new`, then immediately a TLS connection to `new.<p>.lokal` fails (forced and via `check tls`). `.lokal` answers loopback for every name (SPEC §7.2), so "removed" means "no longer served" |
| L18 | Crash recovery | `kill -9 <pid>` → within 2s `status --json` shows `running: true` with a new PID, and L06 passes again |
| L19 | Stop and start | `lokal stop` → `running: false`, the `.local` names stop resolving, and nothing accepts on 443 (so `.lokal` URLs fail too). `lokal start` → L05 and L06 pass within 2s |
| L20 | Upgrade | `lokal upgrade` exits 0. `status.build_id` equals `lokal --version`'s build. L06 passes immediately |
| L21 | `lokal env` | In a subshell after `eval "$(lokal env)"`: Node `fetch` to `https://api.<p>.lokal` succeeds, Python `urllib` to the same URL succeeds, and Python `urllib` to `https://example.com` still succeeds (proving the bundle keeps the system roots). A tool is skipped only if it isn't installed, and example.com only if offline |
| L22 | Full doctor | `doctor --json`: all `dns.*`, `tls.*`, `selftest`, `ca.trusted` and `ca.trust_scope` checks pass, and `upstream.down.<p>.lokal` is `warn` |
| L23 | Performance (`--full` only) | every target in SPEC §14, each measured value reported next to its target |
| L24 | Cleanup and isolation | `project remove <p> --force` (and the `.local` project) exits 0, then immediately every `.local` test FQDN fails `check resolve` and every `.lokal` one fails TLS. `lokal list --json` equals the pre-test snapshot |
| L25 | Safari (optional) | Only if `safaridriver` remote automation is enabled (`safaridriver --enable`, once). Loads the apex, `https://<p>.lokal/`, and sees marker A. It runs before L24, which removes the test project. Otherwise SKIP, which doesn't count as a failure |
| L26 | `lokal run` | Runs after L22. `lokal run lr -p <p> -- echo-server --port {port} --marker R` prints `✓ ready:` on stderr within 5s, and then `curl -sS --fail https://lr.<p>.lokal/` (no `-k`, no `--cacert`) returns marker R on the first try. SIGTERM to lokal: it exits 143 and the echo-server is gone. `lr` is still in `list --json` (the route stays); after `service remove lr`, the same run with `--rm` passes the same checks and leaves `lr` out of `list --json` |
| L27 | `lokal up` | Runs after L26. In a temp folder, a `lokal.toml` with `project = "<p>"` and services `ua` and `ub`, each the echo-server on `{port}` with its own random marker. `lokal up --yes` there prints `✓ ready: 2/2` on stdout within 10s, and then `https://ua.<p>.lokal/` and `https://ub.<p>.lokal/` (curl, no `-k`, no `--cacert`) return their markers on the first try, and `lokal logs ua -p <p> -n 5` exits 0 and shows the echo-server's `LISTENING` line after a timestamp. `lokal restart ua -p <p>` exits 0 and prints `✓ restarted https://ua.<p>.lokal (…)`, and then `https://ua.<p>.lokal/` returns ua's marker on the first request (no retry, no `-k`), from a process that wasn't ua's before. SIGINT to lokal: it exits 130 within 15s, and `pgrep -f <marker>` finds neither echo-server. `ua` and `ub` are still in `list --json` (the routes stay); `lokal down -p <p>` exits 0, both are gone from `list --json`, TLS to `ua.<p>.lokal` fails, and `lokal logs ua -p <p> -n 5` still shows the `LISTENING` line. Afterwards, also on failure, `<log dir>/up/<p>/` is deleted, with the services' logs and the lock in it (the log dir from `lokal internal paths`), and `up/` too if that leaves it empty, so nothing of the run stays in the developer's log folder and L24's isolation check holds |

A healthy run ends with 25 PASS, 0 FAIL and 2 SKIP (L23 without `--full`, and L25 unless Safari automation is on).

## Clean room

`scripts/clean-check.sh` is read-only. Run it after `lokal uninstall` to check that nothing is left behind:

- `security find-certificate -c "lokal development CA" /Library/Keychains/System.keychain` fails;
- the LaunchAgent plist is gone, and `launchctl print gui/$UID/dev.lokal.daemon` fails;
- `lsof -nP -iTCP:443 -sTCP:LISTEN` shows nothing from lokal;
- `/etc/resolver/lokal` and the daemon's copy of the binary are gone;
- no `dev.lokal.verify.*` agents are left;
- `state.toml` and the CA files still exist, because a plain uninstall keeps them.

Before a release, the full cycle is: `lokal uninstall`, then `scripts/clean-check.sh`, then `lokal setup`, then
`scripts/live-test.sh --full`, with the report saved to `docs/live-test-reports/`.

## Release checks

The release workflows ([`RELEASING.md`](RELEASING.md)) run every CI check first, then add their own:

- **`release.yml`:**
  - the tag matches the version in `Cargo.toml`;
  - each binary is built for the right architecture (`lipo -archs`) and its ad-hoc signature verifies
    (`codesign --verify --strict`);
  - each tarball matches its checksum, and the extracted binary prints `lokal <version> (…)`. The Intel binary
    runs under Rosetta;
  - `CHANGELOG.md` has a section for the version.
- **`homebrew.yml`:**
  - the generated formula parses (`ruby -c`);
  - on a fresh runner, `brew install`, `brew test` (which runs `lokal --version`) and
    `brew audit --strict --online` pass, and the installed version is the release.

## VERIFY experiments

`crates/testkit/src/bin/verify_sockets.rs` and `verify_mdns.rs` are the throwaway experiments that proved the
macOS behaviour lokal relies on (SPEC §12): launchd socket activation for ports 443 and 80 in a user
LaunchAgent, and local-only mDNS records being visible to `getaddrinfo`, browsers and curl with no privacy
prompt. `verify_lsof_tree.rs` (V9) shows that `ps -A -o pid=,ppid=` gives a process tree lokal can walk, and
that `lsof -a -p <pids> -iTCP -sTCP:LISTEN -Fn` lists exactly those processes' TCP listeners, which `lokal run`'s
wrong-port hint relies on; it needs no sudo. `verify_process_groups.rs` (V10) shows what "stopped" means for
`lokal up`'s shells: `/bin/sh -c 'sleep 30 & exec sleep 31'` started in a new process group puts both sleeps in a
group whose id is the shell's pid; `killpg(pgid, SIGINT)` ends the leader but not the background `sleep`, which
starts with SIGINT ignored; `killpg(pgid, 0)` returns 0 while the leader is an unreaped zombie and after it is
reaped, and fails with `ESRCH` 5–7ms after `killpg(pgid, SIGTERM)`. A group whose only member is the unreaped
zombie leader gives `EPERM`, not `ESRCH`, so lokal counts `EPERM` as alive and reaps each shell before it calls
the group empty. Measured on macOS 27.0.1 (26A434) with `/bin/sh` being bash 3.2.57, 3 runs; no sudo.
`verify_lsof_port_owners.rs` (V11) shows that `lsof -nP -iTCP:<port> -F pgcnT` lists each process with a socket
on a port once, with its process group (equal to `getpgid` and `ps`), its full command name and each socket's
state, including a socket that is only bound (`CLOSED`, which a connect doesn't see) and connections from or to
the port; that a server with `SO_REUSEADDR` binds a connection's local port, so lokal counts only unconnected
sockets as holding a port; and, with a port argument, that another user's listener shows nothing in lsof while a
connect succeeds; no sudo. They're kept because they document the
findings. The measurements are in
[`history/PROGRESS.md`](history/PROGRESS.md#verify-results).
