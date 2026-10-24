---
title: Performance
description: lokal's performance targets, the numbers its live test measured on macOS 27, and how to reproduce them.
section: concepts
order: 3
source: [docs/SPEC.md, docs/TESTING.md, docs/live-test-reports/macos-27.0-lokal-tld-g2-full.json, crates/testkit/src/bin/live-test.rs]
---

Speed is part of what lokal promises. When `lokal service add` returns, the URL works, and lokal adds almost
nothing to each request. The specification sets a target for each part of that, and the live test measures every
one of them on a real Mac, in check L23.

## Results

| Metric | Target | Measured |
|---|---|---|
| `service add` wall time, from CLI start to ready (p95 of 10) | under 300ms | 13.68ms |
| First request right after `service add` returns | succeeds on the first try | ok |
| Name lookup (`getaddrinfo`, A and AAAA), first | under 20ms | 1.25ms |
| Name lookup, repeated | under 5ms | 0.53ms |
| Proxy overhead on keep-alive requests vs. the upstream directly (p95) | under 2ms | 0.19ms |
| New TLS connection with a resumed session (p95) | under 10ms | 1.67ms |
| Background service start to serving every name (`lokal restart`) | under 500ms | 104.45ms |
| `lokal list` (p95 of 10) | under 30ms | 6.54ms |
| `lokal status` (p95 of 10) | under 30ms | 6.44ms |
| Background service idle CPU, averaged over 10 seconds | under 0.5% | 0.00% |
| Background service memory (RSS) with 20 services | under 30 MB | 17.62MB, with 30 services |

Every target passed, and after the restart every name was served again. This is the raw L23 output from the
report:

```text
service add p95 13.68ms (< 300ms) ok
first request after add: ok
lookup first 1.25ms (< 20ms) ok
lookup repeat 0.53ms (< 5ms) ok
proxy overhead p95 0.19ms (< 2ms) ok
resumed TLS p95 1.67ms (< 10ms) ok
lokal list p95 6.54ms (< 30ms) ok
lokal status p95 6.44ms (< 30ms) ok
RSS with 30 services 17.62MB (< 30MB) ok
idle CPU over 10s 0.00% (< 0.5%) ok
restart → serving all names 104.45ms (< 500ms) ok
all names served after restart: ok
```

## Test conditions

The numbers come from the saved report
[`macos-27.0-lokal-tld-g2-full.json`](https://github.com/mehmetkose/lokal/blob/main/docs/live-test-reports/macos-27.0-lokal-tld-g2-full.json),
the final clean-room run for v0.1 with `.lokal` as the default TLD.

| | |
|---|---|
| macOS | 27.0 |
| lokal | `lokal 0.1.0 (70ec3e3 2026-10-10T14:11:11Z)` |
| Date | 2026-10-10, after a full `lokal uninstall`, a clean-room check and a fresh `lokal setup` |
| Mac | The maintainer's Apple Silicon Mac. The report doesn't record the model |
| Command | `scripts/live-test.sh --full` |
| Test projects | `lt-0eb918` under `.lokal`, plus `lt-0eb918-m.local` for the `.local` path |
| Trust | Verified by macOS. No probe passed `--cacert`, `-k` or `--insecure` |
| Outcome | 24 PASS, 0 FAIL, 1 SKIP (L25, the optional Safari check), in 56.12036 seconds |

Your numbers will differ with your hardware and what else is running. The targets are the contract.

## What each metric means

- **`service add` wall time.** The live test adds 10 services one after another and times each whole
  `lokal service add` process, from start to exit. That includes starting the CLI, writing `state.toml`, and
  waiting while the background service issues the certificate, installs the route and publishes the name.
- **First request after `service add`.** Right after each of those 10 commands returns, the test fetches the new
  URL over HTTPS, verified by macOS. The first attempt must succeed. There's no retry and no sleep.
- **Name lookup.** A `getaddrinfo` call for a name that was just added must return both `127.0.0.1` and `::1`.
  The test times the first lookup and a repeat.
- **Proxy overhead.** 500 keep-alive requests through `https://<name>.lokal/`, and 500 straight to the upstream
  on `http://127.0.0.1:<port>/`. The metric is the difference between their p95 times: what lokal adds to a
  request on an open connection.
- **Resumed TLS connection.** 50 TLS handshakes in a row. The first is a full handshake, and the metric is the p95
  of the other 49, which resume the session. This is what a browser pays for a new connection to a name it has
  already visited.
- **`lokal list` and `lokal status`.** The wall time of 10 runs each, as a script or a coding agent would call
  them.
- **Memory.** The background service's resident memory (`ps -o rss=`), after the test adds 10 more services. The
  report counts every configured service, including the test project's other services and the developer's own
  projects, which is why it says 30.
- **Idle CPU.** The CPU time the background service used during 10 idle seconds, as a percentage.
- **Restart.** The wall time of `lokal restart`, which returns once the new process answers, followed by a check
  that every name resolves and serves HTTPS again.

## Other timings from the same run

Other checks in the same report record times too:

| Check | What it measured | Result |
|---|---|---|
| L04 Create | The `ready in` time each `service add` printed | `api 6ms, web 5ms, down 7ms, @ 6ms` |
| L05 Resolution | First and repeated lookups, in ms | `api.lt-0eb918.lokal 2.514/1.021ms`, and `api.lt-0eb918-m.local 4.308/2.458ms` for `.local` |
| L18 Crash recovery | `kill -9` on the background service until a new process answers | 199ms |
| L19 Stop and start | `lokal start` until names resolve and HTTPS works again | 158ms |
| L20 Upgrade | `lokal upgrade` until the new build answers | `ready in 1.2s` |

## Reproduce it

The live test runs against the lokal installed on your Mac. You need:

- lokal set up on the Mac (`lokal status --json` reports `"setup_done": true`);
- a clone of the repository and Rust 1.89 or later, because the script builds the workspace;
- Google Chrome, for the real-browser check. Node and Python are optional: the `lokal env` check skips a tool
  that isn't installed.

```console
$ git clone https://github.com/mehmetkose/lokal
$ cd lokal
$ scripts/live-test.sh --full
```

`--full` adds L23, the performance check. Without it, L23 is skipped. The test prints one line per check
(`PASS`, `FAIL` or `SKIP`, the check's ID, a description, its duration and detail), writes
`target/live-test-report.json`, and exits 1 if any check fails.

> **Warning:** The live test switches your background service to the build it tests (`lokal upgrade`), and it
> stops, restarts and kills the service along the way, so your lokal URLs blip during the run. It adds and
> removes its own test projects, and fails if your own projects change. When it's done, run `lokal upgrade` with
> your installed `lokal` to switch the background service back.

[TESTING.md](https://github.com/mehmetkose/lokal/blob/main/docs/TESTING.md) describes every check in the live
test and the other test layers.
