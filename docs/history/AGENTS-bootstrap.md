# AGENTS.md: how to build lokal (bootstrap prompt)

> **Historical record.** This is the instruction file a coding agent followed to build lokal v0.1 from scratch on
> the maintainer's Mac, together with [`../SPEC.md`](../SPEC.md). It is kept because it documents how lokal was
> built and tested: human gates for the two privileged steps, VERIFY experiments before building on macOS
> behaviour, and a live test against the installed service. The build log is [`PROGRESS.md`](PROGRESS.md).
>
> For working on lokal today, read [`CONTRIBUTING.md`](../../CONTRIBUTING.md) instead.
> Paths below refer to the layout at the time (`SPEC.md` and
> `PROGRESS.md` were at the repository root). The live test table in §4 is maintained in
> [`../TESTING.md`](../TESTING.md).

You are building `lokal`, a macOS CLI written in Rust that gives local development services real `.local` domains with valid HTTPS. **`SPEC.md` is the source of truth.** Read all of it before writing code, and re-read the relevant sections before each milestone.

You are finished when the product works **on this Mac, installed for real**, as proven by the live test in §4. Compiling is not enough, and neither are passing unit tests. Build, upgrade the installed service, run the live test, read the logs, fix, and repeat until every acceptance criterion in SPEC §15 passes.

## 0. Before writing code

Check the prerequisites, and ask the developer to fix anything missing:
- `rustc --version` (stable, via rustup);
- `xcode-select -p` (Command Line Tools);
- `git status` (a git repo);
- `/Applications/Google Chrome.app`, needed for L07.

Node and Python 3 are optional; L21 skips without them.

Then read SPEC.md completely, create PROGRESS.md, and start M0.

The developer's intended flow is: **base structure (M0) → onboarding slice (M1) → they run `lokal setup` (G1) → you finish M2–M4 in the loop on your own → final clean room (G2).** Expect to stop for the developer twice, at G1 and G2, plus any genuine blocker under §6 rule 8.

## 1. Where you run, and what you may touch

You run directly on the developer's Mac. **No VMs** (there is no disk space for them). That makes the boundaries below strict.

**You never:**
- run `sudo` or anything that asks for a password or shows a GUI authorisation dialog;
- add, remove or edit keychain items or trust settings (`security add-trusted-cert`, `remove-trusted-cert`, `delete-certificate`, `authorizationdb`, `trust-settings-*`);
- write to `/etc`, `/Library`, `/usr/local` or `/System`, or load anything into the `system` launchd domain;
- touch launchd labels other than `dev.lokal.daemon` and your own throwaway `dev.lokal.verify.*` labels, which you must `bootout` and delete when an experiment ends;
- kill processes you didn't start, except the lokal daemon;
- modify or delete the developer's own lokal projects. The live test snapshots them and asserts they are unchanged (L24).

**You may freely:**
- build, run tests, and use read-only system commands (`security find-certificate`, `security verify-cert`, `dscacheutil`, `scutil --dns`, `lsof`, `log show`, `launchctl print`, `ps`);
- run `./target/release/lokal` with any non-sudo command, including `setup` on an already-trusted machine (with `LOKAL_FORBID_SUDO=1`), `upgrade`, `start`, `stop`, `restart`, `project` and `service`;
- bootstrap and boot out `dev.lokal.daemon` and `dev.lokal.verify.*` in `gui/$UID`;
- register `.local` names through `dns_sd`. LocalOnly names are invisible outside this Mac. Use random `lt-…`/`v2-…` names and remove them afterwards.

The installed background service is shared with the developer's real use. Each `lokal upgrade` you run replaces it. That is expected during development, but **never leave it broken at the end of a session**: finish on a build that passes the live test, or on the last one that did.

Disk usage:
- use a single `target/` directory, and don't install extra toolchains;
- put `[profile.dev] debug = "line-tables-only"` in the workspace `Cargo.toml`;
- if `target/` grows past about 10 GB, run `cargo clean -p lokal -p testkit` rather than a full clean.

## 2. Human gates

Only the developer can do privileged steps. When you reach a gate, stop and ask. Write the exact commands in your message and in PROGRESS.md, then wait.

**G1: first trust (end of M1).** This happens once the onboarding slice is complete and the agent has proven everything except the sudo step on this Mac with `setup --skip-trust` (SPEC §15 M1). Run `lokal uninstall` after that proof, so G1 starts from a clean state.
1. Run `./target/release/lokal setup --dry-run` and paste its output into your message, so the developer sees exactly what sudo will run.
2. Ask the developer to run **in their own Terminal**: `./target/release/lokal setup`.
3. Ask them to report:
   - whether any macOS dialog appeared besides the password prompt (VERIFY V4);
   - the final output, or a screenshot.
4. Stop and wait. The developer replies when setup is done, usually by pasting the output.
5. After G1, everything is unprivileged again. Finish M1 (VERIFY V3, V5, V6; L01, L02), then continue through M2–M4 in the loop without further stops until G2.

**G1 again** is needed only if:
- the CA must change (any field in SPEC §6.1, e.g. V3 forces removing the name constraints);
- trust was removed;
- a fallback adds a privileged setup step (Appendix B, or §8.1 (c)).

Avoid these. Batch everything that needs a re-trust into one request.

**G2: final clean room (end of M4).**
1. The developer runs `lokal uninstall`, entering their password.
2. You run `scripts/clean-check.sh` (read-only).
3. The developer runs `lokal setup`.
4. You run `scripts/live-test.sh --full` and save the report to `docs/live-test-reports/`.

## 3. The loop

```
1. cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace            # unit + integration: LOKAL_HOME sandbox, high ports, no system changes
2. cargo build --release --workspace
3. ./target/release/lokal upgrade    # after G1. Swaps the daemon binary, no sudo
4. scripts/live-test.sh              # after G1; checks for features not built yet report SKIP "not yet implemented (Mx)"
                                     # until their milestone. From M3 on, every check must run
5. On any failure:
     ./target/release/lokal doctor
     ./target/release/lokal logs -n 200
     log show --last 2m --predicate 'process == "mDNSResponder"'   # for name issues
   Write the hypothesis in PROGRESS.md, fix it, add a regression test, and go back to 1
6. All green → commit → next criterion
```

Do not skip steps for "small" changes.

## 4. Live test specification

`crates/testkit/src/bin/live-test.rs` is the runner. `scripts/live-test.sh` builds the testkit and runs it, passing its arguments through. Write the logic in Rust, not bash: macOS ships bash 3.2, and the timings need precision.

The `check` binary provides these probes. They verify TLS with `rustls-platform-verifier`, so they trust exactly what macOS trusts:
- `check resolve <host>`: `getaddrinfo`, prints the addresses and the elapsed time
- `check tls <host>`
- `check ws <url>`
- `check sse <url>`
- `check bench <url> -n N`: keep-alive latency distribution

`--cacert <file> --connect <ip:port>` exist **only** for the sandbox integration tests. The live test must never pass them.

Runner rules:
- Use a random project name `lt-<6 chars>`, and free ports found by binding to port 0.
- **Readiness checks do not poll.** The CLI promises the URL works when the command returns, so the first attempt must succeed. Only crash-recovery and stop/start checks poll, every 50ms up to their stated limit.
- Before starting, snapshot `lokal list --json` without the test project. Always clean up the test project and upstreams, even on failure.
- Print one line per check: `PASS|FAIL|SKIP <id> <description> (<duration>) [detail]`.
- Write `target/live-test-report.json`, and exit 1 on any FAIL.
- `--full` also runs L23 (performance).

| ID | Check | Pass condition |
|---|---|---|
| L01 | Setup is idempotent without sudo | `LOKAL_FORBID_SUDO=1 lokal setup --yes` exits 0 and reports that everything is already done |
| L02 | Baseline doctor | `doctor --json` has no `fail` (warnings from the developer's own projects are fine), and `selftest` passes |
| L03 | Upstreams | echo-server A on `127.0.0.1:<a>`, B on `::1:<b>` **only**, D on `127.0.0.1:<d>`; port C stays closed |
| L04 | Create | `project create <p>`; `service add api <a>`; `service add web <b>`; `service add down <c>`; `service add @ <a>`. Each exits 0 and prints `ready in` |
| L05 | Resolution, first try | right after L04: `check resolve` for every FQDN returns **both** 127.0.0.1 and ::1 on the first attempt, first lookup < 20ms, repeat < 5ms |
| L06 | HTTPS via system trust | `check tls api.<p>.local` passes. `curl -sS --fail https://api.<p>.local/` with **no** `-k` and no `--cacert` returns marker A. If V6 showed `/usr/bin/curl` ignores the keychain, run this curl with `--cacert "$(lokal ca path)"` and note it; `check tls` remains mandatory |
| L07 | Real browser | Google Chrome headless with a fresh `--user-data-dir`: `--headless=new --dump-dom --virtual-time-budget=3000 https://web.<p>.local/`. Output contains marker B (which proves the `::1`-only upstream works through `localhost`) and no `ERR_`. **Never** pass `--ignore-certificate-errors` |
| L08 | macOS trust evaluation | `security verify-cert -c <leaf from openssl s_client -showcerts> -p ssl -s api.<p>.local -L` exits 0 |
| L09 | Apex | `https://<p>.local/` returns marker A |
| L10 | Redirect | `curl -sI http://api.<p>.local/x?y=1` → `308`, `Location: https://api.<p>.local/x?y=1` |
| L11 | WebSocket | `check ws wss://api.<p>.local/ws` passes |
| L12 | SSE | `check sse https://api.<p>.local/sse` receives 3 events, each within 300ms of being sent |
| L13 | Upstream down | `https://down.<p>.local/` → 502, and the body mentions `localhost:<c>` |
| L14 | Unknown names | TLS to unconfigured `nope.<p>.local` (forced with `curl --resolve nope.<p>.local:443:127.0.0.1`) fails the handshake. HTTP to it on port 80 returns 404 |
| L15 | Hot add | `service add new <d>`, then **immediately** `https://new.<p>.local/` returns marker D on the first try. The daemon PID is unchanged |
| L16 | Hot update | `service update api --port <d>`, then immediately `https://api.<p>.local/` returns marker D on the first try |
| L17 | Hot remove | `service remove new`, then immediately a TLS connection to `new.<p>.lokal` fails (forced and via `check tls`). `.lokal` answers loopback for every name (SPEC §7.2), so "removed" means "no longer served" |
| L18 | Crash recovery | `kill -9 <pid>` → within 2s `status --json` shows `running: true` with a new PID, and L06 passes again |
| L19 | Stop and start | `lokal stop` → `running: false`, the `.local` names stop resolving, and nothing accepts on 443 (so `.lokal` URLs fail too). `lokal start` → L05 and L06 pass within 2s |
| L20 | Upgrade | `lokal upgrade` exits 0. `status.build_id` equals `lokal --version`'s build. L06 passes immediately |
| L21 | `lokal env` | In a subshell after `eval "$(lokal env)"`: `node -e "fetch('https://api.<p>.local')…"` succeeds; Python `urllib` to the same URL succeeds; Python `urllib` to `https://example.com` still succeeds (proving the bundle keeps the system roots). SKIP a tool only if it isn't installed, or for example.com only if offline |
| L22 | Full doctor | `doctor --json`: all `dns.*`, `tls.*`, `selftest` and `ca.trusted` checks pass, and `upstream.down.<p>.local` is `warn` |
| L23 | Performance (`--full` only) | every target in SPEC §14. Report each measured value next to its target |
| L24 | Cleanup and isolation | `project remove <p> --force` (and the `.local` project) exits 0, then immediately every `.local` test FQDN fails `check resolve` and every `.lokal` one fails TLS. `lokal list --json` equals the pre-test snapshot |
| L25 | Safari (optional) | Only if `safaridriver` remote automation is enabled (the developer can opt in once with `safaridriver --enable`). Loads `https://api.<p>.local/` and sees marker A. Otherwise SKIP, which does not count as a failure |

**`scripts/clean-check.sh`** (G2, read-only) asserts all of the following:
- `security find-certificate -c "lokal development CA" /Library/Keychains/System.keychain` fails;
- the plist is gone;
- `launchctl print gui/$UID/dev.lokal.daemon` fails;
- `lsof -nP -iTCP:443 -sTCP:LISTEN` shows nothing from lokal;
- `/etc/resolver/lokal` is gone;
- the daemon binary is gone;
- no `dev.lokal.verify.*` agents are left;
- `state.toml` and the CA files still exist, since a plain uninstall keeps them.

## 5. VERIFY first

SPEC §12 lists assumptions about macOS that must be proven on this Mac before you build on them.

**V1 and V2 come first, in M0.** You can run both alone. Write tiny throwaway experiment binaries under `crates/testkit/src/bin/verify_*.rs`, and keep them, because they document the findings. Record the commands, macOS version (`sw_vers`) and measurements, then choose the main path or the fallback.

V3–V6 run right after G1.

If a fallback would need a privileged step (SPEC Appendix B, or §8.1 (c)), stop and ask before implementing it. It changes the onboarding.

## 6. Working rules

1. **Never weaken a test to make it pass.** That means:
   - no `-k`/`--insecure` and no `--ignore-certificate-errors`;
   - no `--cacert` outside the sandbox integration tests and the one documented L06 case;
   - no added sleeps or retries in readiness checks;
   - no loosened targets, and no SKIP for something that can run.
   
   If a target in SPEC is truly unreachable, record the evidence in PROGRESS.md and ask.
2. **Speed is a feature.** If a readiness check only passes with a retry, the bug is in lokal's readiness reporting. Fix the daemon so `reload` waits for the real event (the mDNS callback, the pre-warmed cert); don't change the test.
3. **Keep the platform seam clean** (SPEC §10.1):
   - no `cfg(target_os)` or macOS APIs outside `src/platform/`;
   - `PORTING(linux|windows)` comments on every trait method and every macOS assumption;
   - the crate must still `cargo check` on Linux. CI verifies this if the repo has a GitHub remote; you don't need a Linux machine.
4. **Deviating from SPEC** needs a "Decisions" entry in PROGRESS.md: what the spec said, what you did, why, and the evidence. Never deviate from SPEC §16 (security) without asking.
5. **Every bug gets a regression test** at the lowest level that catches it.
6. **Commit** after each green loop iteration. Use small commits with clear messages, and never commit failing tests.
7. **When stuck:** if the same check fails 3 iterations in a row with no new information, stop changing code. Write at least 3 hypotheses in PROGRESS.md and design one experiment that tells them apart. If 3 more iterations still make no progress, ask the developer, with the hypotheses and evidence.
8. **Ask the developer only** for the gates in §2, a failed VERIFY that has no unprivileged fallback, a security change, or when you're stuck per rule 7.
9. **Don't build M5 features** unless asked.

## 7. PROGRESS.md

Create it at the start and keep it current. It is the developer's view of where things stand. Use this structure:

```markdown
# Progress

## Current milestone: M1 — Onboarding slice
Waiting on: nothing  |  G1 (developer runs `./target/release/lokal setup`)

| Criterion (SPEC §15) | Status | Evidence |
|---|---|---|
| VERIFY V1 launchd sockets on 127.0.0.1:443 | pass | macOS 15.6, verify_sockets, curl ok, no prompt |
| L07 headless Chrome | fail | ERR_CERT_… see iteration 14 |

## VERIFY results
- V2 (macOS 15.6): LocalOnly A+AAAA visible to getaddrinfo in 1.8ms; Shared vs Unique same latency → Unique. Commands: …

## Decisions
- 2026-10-12: SPEC §X said …; did …; because …; evidence …

## Iteration log (newest first)
- #15 2026-10-12: reload now waits for both mDNS callbacks → L05 first-try pass. Run: 23 PASS / 0 FAIL / 2 SKIP (L23 not --full, L25 safaridriver off)
```

## 8. Definition of done (v1)

- M0–M4 acceptance criteria are all checked in PROGRESS.md, with evidence.
- G2 passed, and `scripts/live-test.sh --full` passes with 0 FAIL. The report is saved in `docs/live-test-reports/`.
- CI is green (macOS tests plus the Linux `cargo check`), or PROGRESS.md notes that there is no remote.
- `lokal --help` lists every command (SPEC §4.2).
- The README and `docs/PORTING.md` are written.
- The installed background service runs the final build.
