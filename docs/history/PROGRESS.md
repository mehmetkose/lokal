# Progress (v1 build log)

> **Historical record.** This is the log the coding agent kept while building lokal v0.1 from
> [`../SPEC.md`](../SPEC.md), following [`AGENTS-bootstrap.md`](AGENTS-bootstrap.md). "The developer" is the
> maintainer; G1/G2 are the human gates where they ran `lokal setup`/`uninstall` themselves. The **Decisions**
> section explains every place the build deviated from the spec, with evidence, and the **VERIFY results**
> record what was measured on macOS. File paths refer to the layout at the time (`SPEC.md` and `AGENTS.md` were
> at the repository root). Current status lives in the [changelog](../../CHANGELOG.md).

## Current milestone: v1 + `.lokal` done; CA narrowed to `.lokal` + `.local`, no IP addresses (G1 again passed)
Waiting on: nothing. Developer ran `lokal setup` 2026-10-17 (new CA F4D7ACB0… trusted, old F0AAA12F… removed);
live test 23 PASS / 0 FAIL / 2 SKIP (iteration #15). Installed service runs the current build.

Final evidence (`.lokal` default, `.local` kept): G2 clean-check 10/10 PASS (2026-10-10 14:18:26Z, incl.
`/etc/resolver/lokal` gone), developer `lokal setup`, then `scripts/live-test.sh --full` → 24 PASS / 0 FAIL / 1 SKIP
(L25 Safari optional). Report: docs/live-test-reports/macos-27.0-lokal-tld-g2-full.json. Installed service runs the
final build.

| Criterion (SPEC §15) | Status | Evidence |
|---|---|---|
| M0 workspace + layout §10.2, platform traits, macOS impl, unsupported.rs | pass | `cargo check -p lokal --target x86_64-unknown-linux-gnu` ok (unsupported.rs compiles) |
| M0 `target_os` scan test | pass | `crates/lokal/tests/seam.rs` (also checks PORTING notes on every trait method) |
| M0 docs/PORTING.md | pass | Appendix A + macOS specifics |
| VERIFY V1 launchd sockets on 127.0.0.1:443/::1:443/:80 | **pass** | macOS 27.0 (26A428), arm64; `verify_sockets`; curl ok on all 4; no prompt |
| VERIFY V2 dns_sd LocalOnly A+AAAA | **pass** | macOS 27.0; `verify_mdns`; getaddrinfo/dscacheutil/curl/Chrome ok; no Local Network prompt (also from a LaunchAgent) |
| M0 CI green | n/a | no GitHub remote: "CI not yet run (no remote)"; workflow written (`.github/workflows/ci.yml`) |
| M1 CA exactly per §6.1, tested with x509-parser | pass | `ca::tests::ca_fields_match_spec`; `openssl x509 -text` of the real CA matches |
| M1 leaf issuance + on-demand resolver | pass | `ca::tests::leaf_fields_match_spec`; unknown SNI → handshake alert |
| M1 daemon: launchd sockets, mDNS names, IPC ping/status/selftest | pass | `status`: listener_source launchd, name_backend mdns-local-only |
| M1 proof 1: `setup --skip-trust` + status | pass | 0.28s; listeners 127.0.0.1:443, [::1]:443, 127.0.0.1:80, [::1]:80 (launchd) |
| M1 proof 2: selftest (except macOS trust) | pass | `https://lokal-selftest-<hex>.local verified with the CA file` 10–17ms |
| M1 proof 3: upgrade / restart / kill -9 | pass | upgrade 158ms; restart 65–70ms; kill -9 → new PID in 8ms (1.0s if the killed daemon was <1s old: ThrottleInterval) |
| M1 proof 4: uninstall leaves no LaunchAgent | pass | `LOKAL_FORBID_SUDO=1 lokal uninstall --yes` (no sudo needed); clean-check all PASS |
| G1 | pass | developer ran `lokal setup` 2026-10-10 11:49Z; `internal trust-status` → Trusted; setup selftest `GET lokal-selftest-4bcf1f.local 200` |
| M1 after G1: VERIFY V3 | pass | security verify-cert rc 0; rustls-platform-verifier; Chrome headless; curl; `node --use-system-ca` (plain node fails as expected) |
| M1 after G1: VERIFY V4 | pass (fallback: messaging) | developer: during setup a macOS dialog also asked for the password (besides sudo). Setup/uninstall/README now say so |
| M1 after G1: VERIFY V5 | n/a | Firefox not installed; setup writes the `user.js` pref if it appears (§4.6 fallback) |
| VERIFY V7 (.lokal resolver + DNS server) | pass | getaddrinfo/curl/Chrome/Node/Python/verify-cert; wildcard needed (Decisions) |
| M1 after G1: VERIFY V6 | pass | `/usr/bin/curl` 8.7.1 (SecureTransport) trusts the keychain → L06 curl without --cacert |
| M1 after G1: doctor ca.trusted + selftest | pass | doctor all pass in 64ms |
| M1 after G1: L01, L02 | pass | live run #6 |
| M2 full command tree parses; root help generated + leaf test | pass | `cli::help::tests::*`, `tests/cli.rs::help_lists_every_command_and_has_examples` |
| M2 CRUD, list, current project, every §4.3 rule (integration test per rule) | pass | `tests/cli.rs` (20 tests, LOKAL_HOME) |
| M2 `--json` for list, project list/show, service list per §4.8 | pass | `tests/cli.rs::json_shapes_match_spec` |
| M2 20 parallel `service add` lose no writes | pass | `tests/cli.rs::twenty_parallel_adds_lose_no_writes` |
| M3 sandbox: h1/h2, SSE, WS, 50MB POST, 502, 404, 308, X-Forwarded-*, host_header modes, 421, ::1-only and 127.0.0.1-only | pass | `tests/proxy.rs` (12 tests) |
| M3 unknown / missing SNI fails the handshake | pass | `tests/proxy.rs::unknown_host_404_and_unknown_or_missing_sni_fails` |
| M3 reload returns only when ready; `ready in Nms`; in-flight survive | pass | `tests/proxy.rs::reload_is_ready_on_return_and_in_flight_requests_survive` |
| M3 mdns-local-only: getaddrinfo sees lt-<rand> names on return, gone on removal | pass | `tests/mdns.rs` |
| M3 live L01–L22, L24 | pass | `scripts/live-test.sh --full`: 24 PASS / 0 FAIL / 1 SKIP (L25) — docs/live-test-reports/macos-27.0-pre-g2-full.json |
| M4 README | pass | README.md |
| M4 doctor (all §11 checks, < 1s) | pass | 64ms healthy; per-service dns/dns.conflict/tls/upstream; L22 |
| M4 env, ca path, open, Firefox handling, nudges, welcome | pass | `env` (sh/fish), `ca path`; nudges exit 3 (tests/cli.rs); Firefox code path present (not installed) |
| M4 all §14 performance targets (L23) | pass | add p95 32ms; lookup 3.7/2.3ms; proxy overhead p95 0.23ms; resumed TLS p95 4.1ms; list/status p95 8ms; RSS 17.3MB (24 svcs); idle CPU 0%; restart 139ms |
| L25 Safari (optional) | SKIP | safaridriver: "timed out while connecting to a Safari instance" (Safari can't be driven on this Mac); Safari uses the same Security.framework evaluation as L08 |
| G2 | pass | attempt 2: developer `uninstall` → clean-check 9/9 PASS → developer `setup` → live test --full 24/0/1 (docs/live-test-reports/macos-27.0-g2-final-full.json) |
| CI | n/a | no GitHub remote ("CI not yet run (no remote)"); Linux `cargo check` verified locally with the x86_64-unknown-linux-gnu target |
| M4 perf pre-checks (sandbox) | ok so far | list/status 5ms; RSS 16.6MB with 20 services; idle CPU 0; proxy overhead p95 ≈ 0.1ms |

## VERIFY results

Machine: macOS 27.0 (build 26A428), Apple Silicon (arm64), rustc 1.98.1.

- **V1 (pass, main path).** Throwaway agent `dev.lokal.verify.v1<hex>` in `gui/501` declaring
  `Sockets` https4 `127.0.0.1:443`, https6 `::1:443`, http4 `127.0.0.1:80`, http6 `::1:80`,
  running `target/release/verify_sockets serve`. `launch_activate_socket` returned 1 fd per name;
  `lsof` shows the agent process (user-owned) holding all four listeners;
  `curl -k https://127.0.0.1/`, `https://[::1]/`, `http://127.0.0.1/`, `http://[::1]/` all answered.
  No dialog. Agents booted out and plists deleted afterwards.
  - `kill -9` of the agent: a connection made while it was dead **queued in the launchd-owned
    backlog** and was served by the respawned process. But respawn took 8–9s with the default
    `ThrottleInterval` (10s). With `ThrottleInterval` 1 (or 0) respawn took **7ms** (measured by
    polling `launchctl print` every 50ms, 2 runs each). → plist gets `ThrottleInterval = 1` (Decision below).
  - Fallback (a) checked for the record: an unprivileged `bind(127.0.0.1:444)` and `[::1]:444` →
    **EACCES** on macOS 27. So (a) is not available here; the main path does not need it.
- **V2 (pass, main path, `kDNSServiceFlagsUnique`).** `verify_mdns <unique|shared>` registers
  `v2-<hex>.local` and `api.v2-<hex>.local` (A 127.0.0.1 + AAAA ::1, `kDNSServiceInterfaceIndexLocalOnly`, TTL 120)
  on one `DNSServiceCreateConnection`.
  - All 4 register callbacks: 4.5–23ms (Unique), 4.5–8ms (Shared). Same order of magnitude → keep **Unique** (SPEC default).
  - getaddrinfo (new process, first lookup): 6.7–17ms typical (one cold outlier at 46ms on the very first run);
    second name / repeat: 1.4–7.8ms. Both `::1` and `127.0.0.1` returned for the apex and nested name.
  - `dscacheutil -q host -a name api.v2-holdtest.local` → both addresses. `curl http://api.v2-holdtest.local:18765/` → 200 via 127.0.0.1, namelookup 8ms.
    Chrome headless (`--headless=new --dump-dom`) loaded `http://api.v2-chrome.local:18765/` and printed the marker.
  - Registered from inside a LaunchAgent (`dev.lokal.verify.v2<hex>`): callbacks 9ms, other processes resolve it;
    `log show` has no Local Network / TCC entries. No prompt.
  - Removal: after `DNSServiceRemoveRecord` (returns in ~0.3ms) the very next getaddrinfo fails
    (no stale answer). A negative `.local` lookup takes ~5.0s (multicast timeout; standard macOS behaviour, also for never-registered names).
    Merely closing the connection (`DNSServiceRefDeallocate`) is asynchronous: an immediate lookup once still returned the A record →
    the daemon removes records explicitly before deregistering.
  - A second process registering the same name with Unique on LocalOnly got **no** conflict error (both coexist).
  - Observation for L07: Chrome headless on macOS 27 prints the DOM but does not exit afterwards; the runner must read
    the DOM and then terminate Chrome (not a retry, not a weakened check).

## Decisions
- 2026-10-10: Started v2 from scratch. The developer staged deletion of the previous (multi-platform, never live-tested on macOS)
  implementation and rewrote SPEC/AGENTS for macOS only. Its Cargo.lock was restored so builds use the cached registry
  (6.5 GB free disk).
- 2026-10-10: SPEC §8.1 plist adds `<key>ThrottleInterval</key><integer>1</integer>`. The spec's plist didn't list it;
  without it launchd waits ~10s before respawning a crashed daemon, which breaks L18 (new PID within 2s). Evidence: V1 above.

- 2026-10-10: SPEC §6.3 says `security verify-cert -c <leaf> -r <root.pem> -p ssl -s … -L`. Did `-c <leaf> -c <root.pem>` (root as a
  chain cert, no `-r`). Why: on macOS 27 `-r` makes the root an **anchor**, so verification succeeds even when the keychain does not
  trust it. Evidence (sandbox CA, untrusted): `-r` → "certificate verification successful", rc 0; `-c root` → `CSSMERR_TP_NOT_TRUSTED`, rc 1.
  Bonus evidence for V3: with `-r`, a leaf for `www.google.com` from the name-constrained CA fails with `CSSMERR_TP_INVALID_CERTIFICATE`
  while the `.local` leaf passes → Apple's verifier enforces (and accepts) the name constraints.

- 2026-10-10: Developer asked about `https://anything.lokal` instead of `.local`. Options explained (custom TLD needs a resolver
  + `/etc/resolver/lokal` sudo step, a CA with `lokal` in its name constraints, and a SPEC §16 change). **Developer chose: keep
  `.local` for v1** (SPEC unchanged; CA name constraints stay local/test/internal).

- 2026-10-10: SPEC §4.2's target root help ends at "Quick start". On the developer's request (agents should learn lokal from
  `lokal` alone) the root help adds a short "Using lokal in a project (for people and coding agents)" section, and
  `project create`, `list`, `status`, `env` got long help. Command list is still generated from the clap tree.

- 2026-10-10: Developer request (#1): setup commands are safe to re-run, so agents and scripts can repeat them.
  `project create` on an existing project (same or omitted --domain) succeeds and makes it current; an identical
  `service add` succeeds without a state write and still waits for ready; differing settings fail with the exact
  `service update` command. SPEC §4.3 updated. Tests: cli.rs (project_create_is_safe_to_rerun,
  service_add_is_safe_to_rerun, uniqueness_rule_message), proxy.rs (re-add in the hot-reload test), live L04 re-runs.

- 2026-10-10: Custom TLD `.lokal` experiment (developer asked again, for branding). `dig lokal. SOA @a.root-servers.net`
  → NXDOMAIN (not delegated today). `VERIFY_SUFFIX=lokal verify_mdns unique|shared`: `DNSServiceRegisterRecord` on
  LocalOnly returns -65540 (kDNSServiceErr_BadParam) for `v2-tld.lokal` with both flags → mDNSResponder serves
  LocalOnly records only under `.local`. A `.lokal` TLD would need lokal's own DNS server + `/etc/resolver/lokal` (sudo).

- 2026-10-10: **`.lokal` TLD (developer decision, for branding).** New projects default to `.lokal`; `.local` keeps working.
  Design (SPEC §3.1, §4.6, §7.2, §11, §16 updated with the developer's go-ahead):
  built-in DNS server on a launchd UDP socket 127.0.0.1:53535; `/etc/resolver/lokal` installed by setup in the same
  sudo batch as trust; CA name constraints now include `lokal`, so setup replaces an older CA (old cert retired to
  `ca/retired/<sha1>/`, key deleted, untrusted + deleted from the keychain in that batch); doctor `resolver.lokal`
  and `tld.lokal` (DNS root NXDOMAIN check) added.
- 2026-10-10: **`.lokal` answers loopback for every name** (wildcard, TTL 0) instead of NXDOMAIN for unknown names.
  Evidence (macOS 27, getaddrinfo): a name looked up before `service add` stayed unresolvable for 73.8–75.0s with
  NXDOMAIN(+SOA, min 0), NXDOMAIN without SOA, NODATA(±SOA), SERVFAIL and REFUSED alike — mDNSResponder caches any
  failure ~75s and there is no unprivileged flush. Positive TTL 0 is honoured (removal visible at once). With the
  wildcard, a looked-up-before-add name works immediately. Consequence: a removed `.lokal` name still resolves but is
  not served (TLS handshake refused, HTTP 404). AGENTS.md L17/L19/L24 updated to check "not served" for `.lokal`.
- 2026-10-10: doctor's `tld.lokal` sends the root query with RD set: this network intercepts port 53 and answers
  SERVFAIL (flags 0x8082) to non-recursive queries; root servers ignore RD.
- 2026-10-17: **CA name constraints narrowed to `local` + `lokal`** (developer request, from a security audit). SPEC §6.1
  said `local`, `lokal`, `test`, `internal`. lokal never used `.test`/`.internal`, but the System-keychain trust applies
  to every user of the Mac, so a leaked key could have signed company intranet names under those TLDs. Changes:
  `ca::PERMITTED_TLDS`; `CaCert::is_current` now needs an exact match (`scope_problem` says why), so setup replaces the
  old CA with the existing retire → new CA → one sudo batch flow; doctor `ca.scope` (warn); the validator rejects
  `.test`/`.internal` like any other TLD (no "coming later" message). Also fixed: setup now checks that sudo can run
  (`LOKAL_FORBID_SUDO`, a terminal) **before** changing anything. Before, it retired the CA (deleting the key) and only
  then failed, which would have left an untrusted CA behind (live L01 on an outdated CA). Needs G1 again.
  SPEC §4.3, §4.6, §6.1, §11, §16.9, README, ROADMAP updated.
- 2026-10-17: **CA also excludes every IP address** (`excluded iPAddress 0.0.0.0/0, ::/0`), found while testing the
  narrowing in Chrome. SPEC §6.1 constrained DNS names only, and RFC 5280 constrains each name form separately, so IP
  addresses were unconstrained: a `127.0.0.1` leaf signed by the trusted CA loaded in Chrome as a valid HTTPS page and
  passed `security verify-cert`. "Only `.lokal` and `.local`" (the developer's request) needs the exclusion. Batched
  into the same G1. lokal never issues IP leaves, so nothing it serves changes. Evidence: iteration #14.
- 2026-10-20: **`.lokal` and `.local` only, for good** (developer decision). SPEC §15 M5 listed `.test`/`.internal`
  via the built-in DNS server; that would undo the 2026-10-17 narrowing and need a new CA. Removed from M5; the plan is
  in `planner/discarded/dns-test-internal-tlds.md`. The Linux porting note (SPEC Appendix A, `docs/PORTING.md`) now
  routes `~lokal` instead of `~test` through systemd-resolved.

## Iteration log (newest first)
- #15 2026-10-17: G1 again done by the developer (`lokal setup`). Keychain holds only the new CA (F4D7ACB0…, local +
  lokal, 0.0.0.0/0 + ::/0 excluded); doctor 31 pass, no warnings. Live test 22/1/2: **L24 FAIL**, `current_project`
  "<developer's project>" → null. Cause: the runner's test project becomes current on create (SPEC §4.4) and removing it clears the
  setting; earlier runs passed only because no project was current. Fix: the runner remembers the current project
  and restores it in L24 and in cleanup (comparison unchanged); regression test
  `live-test tests::remembers_the_developers_current_project`; developer's setting restored with `project use`.
  Re-run: 23 PASS / 0 FAIL / 2 SKIP (L23 not --full, L25 safaridriver off). Chrome MCP on the real new CA:
  `https://web.<developer's project>.lokal/` loads (secure context, h2); throwaway 1-day leaves signed by the new CA: `.lokal` and
  `.local` load; `localhost`, `127.0.0.1`, `[::1]` → `ERR_CERT_INVALID` (`security verify-cert`: "Name constraints
  violated"). Keys deleted.
- #14 2026-10-17: Tested the narrowing in Chrome (chrome-devtools MCP, Chrome 156) without touching trust. Throwaway
  1-day leaves served by a Python HTTPS server, keys deleted afterwards. (A) Leaves signed by the **current trusted CA**:
  `lt-066c24.lokal` loads; `localhost` → `ERR_CERT_INVALID` (Chrome enforces the root's constraints); `127.0.0.1` and
  `[::1]` **load as valid HTTPS**, and `security verify-cert` trusts them → IP gap. Fixed: CA excludes 0.0.0.0/0 and
  ::/0, `scope_problem` reports "IP addresses" (`ca::tests::ca_must_exclude_every_ip_address`). (B) New CA from
  `LOKAL_HOME=… lokal internal create-ca`: `security verify-cert -r <new CA>` and `openssl verify` accept `.lokal`,
  `.local` and `api.<p>.local` leaves (from `internal issue-leaf`) and reject `x.test`, `x.internal`, `localhost`,
  `example.com`, `127.0.0.1`, `::1` and `192.168.1.1` ("Name constraints violated"; OpenSSL errors 47/48). (C) In Chrome
  through the real trust path: the new CA cross-signed by the trusted one (same subject, so its pathlen:0 holds;
  nameConstraints bytes identical to the new CA's), 1 day. `.lokal` and `.local` (verify_mdns LocalOnly) load;
  `127.0.0.1`, `[::1]` and `localhost` → `ERR_CERT_INVALID`; control `[::1]` under the old CA still loads. `.test` can't
  resolve in Chrome without sudo (LocalOnly is `.local`-only), so it rests on (B).
- #13 2026-10-17: CA narrowed (see Decisions). fmt/clippy/tests green (new: `ca::tests::ca_with_other_tlds_is_outdated`,
  `setup::tests::sudo_precheck`). On this Mac, without sudo: `setup --dry-run` plans untrust old (F0AAA12F…) + trust new;
  `LOKAL_FORBID_SUDO=1 setup --yes` → exit 4 with root.pem/root-key.pem hashes unchanged and no `ca/retired/`;
  doctor → `ca.scope` warn, everything else as before. Service not upgraded (waits for G1).
- #12 2026-10-10 14:2xZ: G2 for `.lokal`: developer `uninstall` → clean-check 10/10 → developer `setup` → live `--full`
  24 PASS / 0 FAIL / 1 SKIP; §14: add p95 14ms, lookup 1.3/0.5ms, proxy overhead p95 0.19ms, resumed TLS p95 1.7ms,
  restart 104ms, RSS 17.6MB (30 svcs), idle CPU 0%.
- #11 2026-10-10: `.lokal` implemented. Developer ran `lokal setup` (new CA trusted, old removed, resolver active).
  VERIFY V7: `.lokal` resolves via getaddrinfo 1.5ms first / 0.6ms repeat; curl, Chrome headless, `node --use-system-ca`,
  Python (`lokal env`), `security verify-cert` all pass. First attempt answered NXDOMAIN → 75s negative caching found
  (see Decisions) → wildcard. Live `--full`: 24 PASS / 0 FAIL / 1 SKIP (L25), both TLDs covered, all §14 targets met.
  Also fixed: doctor ran dns.* lookups concurrently (inflated to ~25ms → false "slow" warnings); now sequential.
- #10 2026-10-10: setup commands safe to re-run (see Decisions). Bug found while demoing: `lokal … | head -1` panicked
  ("failed printing to stdout: Broken pipe") because Rust ignores SIGPIPE. Fix: CLI restores default SIGPIPE
  (daemon keeps ignoring it). Regression test `closed_stdout_pipe_does_not_panic` fails without the fix, passes with it.
  Live: 23 PASS / 0 FAIL / 2 SKIP.
- #9 2026-10-10: VERIFY V4 answered by the developer: `sudo security add-trusted-cert -d …` also opens a macOS
  authorization dialog asking for the password. Per SPEC §12 fallback, only messaging changed (setup plan line, trust
  step, uninstall plan, README).
- #8 2026-10-10 12:01Z: G2 attempt 2 passed: watcher polled every 1s and ran clean-check the moment uninstall finished
  (9/9 PASS), developer ran setup, final `--full` live test 24 PASS / 0 FAIL / 1 SKIP; all §14 targets met
  (add p95 18ms, lookup 4.5/3.0ms, proxy overhead p95 0.21ms, resumed TLS p95 4.5ms, restart 131ms, RSS 17.4MB, idle CPU 0%).
- #7 2026-10-10 11:57Z: G2 attempt 1. Developer ran `uninstall`, then `setup`, then `scripts/live-test.sh --full` →
  **24 PASS / 0 FAIL / 1 SKIP (L25)**, report docs/live-test-reports/macos-27.0-after-g2-full.json.
  clean-check did not get a clean run: the agent's watcher saw the uninstall (plist gone AND
  `security find-certificate -c "lokal development CA" System.keychain` failing), slept 3s, and by then `setup` had already
  re-trusted the CA (trust step precedes the service step, sudo cached) → 8/9 PASS, "CA not in keychain" FAIL.
  Watcher bug (sleep before checking), not an uninstall bug. Asked the developer to repeat uninstall → clean-check → setup.
- #6 2026-10-10: after G1, first live run 23 PASS / 1 FAIL (L25: safaridriver hung 30s creating a session, runner read
  timeout 30s) / 1 SKIP. Fix: runner waits 60s and SKIPs when safaridriver can't create a session (before any page loads).
  `--full`: 24 PASS / 0 FAIL / 1 SKIP; all §14 targets met.
- #4 2026-10-10: CLI rule tests (20), sandbox engine tests (12), mDNS readiness test, live-test runner, README. Runner dry-run
  against a `--skip-trust` install: non-TLS checks pass; TLS checks fail as expected (UnknownIssuer / Chrome
  `ERR_CERT_AUTHORITY_INVALID`). Fixed: `check resolve` no longer repeats a failed lookup (2×5s); L07 reads Chrome's
  stderr for `Page load failed`. Uninstalled again → clean state for G1 (clean-check PASS).
- #3 2026-10-10: M1 slice proven on the real Mac without sudo (see table). Real mDNS through the installed daemon: `service add`
  ready in 7–10ms, getaddrinfo first 4–5ms / repeat 1.6ms, both ::1 and 127.0.0.1; removal → next lookup fails at once.
  Pre-G1 V3 evidence: Node (OpenSSL) and Python 3.14 (OpenSSL 3.6.4) accept the name-constrained chain (TLS 1.3).
- #2 2026-10-10: M0 complete. Whole engine smoke-tested in a sandbox (h1/h2, WS, SSE lag 1ms, ::1-only upstream via localhost,
  502/404/308, unknown SNI refused, `service add` 29ms wall, proxy overhead p95 ≈ 0.12ms over direct).
- #1 2026-10-10: VERIFY V1 + V2 done (both main paths). Workspace created.
