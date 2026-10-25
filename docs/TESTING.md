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
- **`crates/lokal/tests/cli.rs`** has one test per validation rule in SPEC §4.3, plus the `--json` shapes and
  20 parallel `service add` calls that must lose no writes. All run under `LOKAL_HOME`.
- **`crates/lokal/tests/proxy.rs`** runs the real daemon in a sandbox
  (`lokal daemon run --foreground --https-port <p> --http-port <q> --name-backend none`) with a sandbox CA. It
  covers HTTP/1.1 and h2, SSE timing, WebSockets, a 50 MB streamed POST, 502/404/308/421, `X-Forwarded-*`, both
  `host_header` modes, `::1`-only and `127.0.0.1`-only upstreams, unknown or missing SNI, and reload readiness.
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
  not a listener elsewhere in lokal's process group (cargo test runs everything in one group); and the notes when
  no daemon runs. Two tests count the signals that
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
  clashes, domains, busy fixed and kept ports, and the other commands on managed services. Against the sandbox
  daemon, every URL answers with the 502 page as soon as `up` returns and fails the TLS handshake as soon as `down`
  returns, and `lokal run` on a managed service keeps its marker. The parsing rules and the port plan (with a fake
  probe and port picker) have unit tests in `project_file.rs` and `cli/up.rs`.
  - **The trust check** (SPEC §16.11). With no terminal (stdin closed) and no `--yes`, `up` exits 1, prints the
    path and every command, leaves `state.toml` byte-identical, writes no approval, and neither stream contains
    `--yes`. With `--yes` (or `-y`) it prints the listing and its note, routes, and leaves `<LOKAL_HOME>/trusted/`
    empty. Through a pty (lokal leads a session with it as its controlling terminal, the helper in
    `tests/common/mod.rs` that `run.rs` uses too), `n`, an empty answer and Ctrl-D exit 1 with nothing routed and
    no record, and `y` exits 0 and writes the record: 0600 in a 0700 directory, named by the SHA-256 of the file's
    real path, holding the SHA-256 of its bytes and a newline. After that, a run with no terminal needs nothing;
    after a comment is appended, it exits 1 saying the file changed, and `--yes` leaves the old record alone. A
    symlinked repo path uses the same approval, and a copy of the file elsewhere asks again. The tests that aren't
    about the trust check pass `--yes`. Unit tests cover the decision table, the record (a match, another digest,
    missing, unreadable), the digest and real path that `load` takes, and the listing, including that no control
    or invisible character in the file reaches the terminal.
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

A healthy run ends with 24 PASS, 0 FAIL and 2 SKIP (L23 without `--full`, and L25 unless Safari automation is on).

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
wrong-port hint relies on; it needs no sudo. They're kept because they document the findings. The measurements are in
[`history/PROGRESS.md`](history/PROGRESS.md#verify-results).
