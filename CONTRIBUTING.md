# Contributing to lokal

Thanks for helping. Bug reports, fixes, docs, and especially reports from Macs and setups the maintainer hasn't
tested are all welcome.

By taking part you agree to the [Code of Conduct](CODE_OF_CONDUCT.md). Security issues go through
[`SECURITY.md`](SECURITY.md), not public issues.

## Before you start

- **Small fixes** (typos, docs, obvious bugs): open a pull request directly.
- **Bigger changes** (new commands, behaviour changes, anything in the proxy, DNS, CA or setup): open an issue
  first, so you don't spend time on something that conflicts with the design.
- **[`docs/SPEC.md`](docs/SPEC.md) is the source of truth for behaviour.** If your change alters what lokal does,
  update the spec in the same pull request. Planned features are in [`docs/ROADMAP.md`](docs/ROADMAP.md).
- **Security requirements** ([SPEC §16](docs/SPEC.md#16-security-requirements-non-negotiable)) are not negotiable
  in a pull request. If you think one should change, open an issue and explain why.

## Set up

You need macOS, [Rust](https://rustup.rs) stable (1.89 or later) and the Xcode Command Line Tools.

```console
$ git clone https://github.com/mehmetkose/lokal
$ cd lokal
$ cargo build --workspace
```

The workspace has two crates:

- `crates/lokal`: the CLI and the background service (one binary).
- `crates/testkit`: dev-only tools that are never shipped: an echo server, the `check` probes, the live test
  runner, and the `verify_*` experiments that proved the macOS assumptions.

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) maps the modules.

## Test

Run these before you push. CI runs the same checks:

```console
$ cargo fmt --all -- --check
$ cargo clippy --workspace --all-targets -- -D warnings
$ cargo test --workspace
```

`cargo test` needs no setup and no sudo, and it doesn't touch your installed lokal. The integration tests run the
daemon in a sandbox (`LOKAL_HOME=<temp dir>`, high ports, a throwaway CA). On macOS, `tests/mdns.rs` registers
random `lt-<hex>.local` names with mDNSResponder on the local-only interface. Nothing outside your Mac can see them,
and they're removed when the test ends.

lokal ships for macOS only, but CI also runs clippy and the tests on Linux (x86_64 and arm64), where the pieces
that need no privileges are ported ([`docs/PORTING.md`](docs/PORTING.md)). Lint the Linux code on your Mac too. It
needs the rustup target, but no linker, and takes a few seconds:

```console
$ rustup target add x86_64-unknown-linux-gnu
$ cargo clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```

### The live test

Unit and sandbox tests can't prove that a browser trusts the certificate or that macOS resolves the name. The
live test does, against the lokal installed on your Mac:

```console
$ make install && lokal setup     # once, if lokal isn't set up yet
$ scripts/live-test.sh            # about 25 checks; --full adds the performance targets
```

It needs Google Chrome for the browser check. Node and Python 3 are optional: their checks are skipped if they
aren't installed.

The live test creates a random `lt-xxxxxx` project, checks it from every angle (DNS, TLS through macOS trust,
Chrome headless, curl, WebSocket, SSE, crash recovery, upgrade) and removes it. It snapshots your own projects
first and fails if they change. It also runs `lokal upgrade`, so afterwards your background service runs the
build you tested.

Run it when you change anything in the proxy, the daemon, DNS, certificates or setup. Paste the summary line in
your pull request. [`docs/TESTING.md`](docs/TESTING.md) describes every check.

## Rules for code changes

- **Every bug fix comes with a regression test**, at the lowest level that catches it.
- **Never weaken a test to make it pass.** That means no `-k`, `--insecure` or `--ignore-certificate-errors`, no
  sleeps or retries in readiness checks, and no loosened performance targets. If a URL works only after a retry,
  the bug is in lokal's readiness reporting.
- **Keep the platform seam clean.** No `cfg(target_os)` or macOS API outside `crates/lokal/src/platform/`, and a
  `PORTING(linux|windows)` comment on every platform trait method. `tests/seam.rs` enforces both.
- **Run other programs only through `process::run`**, with an argument vector. Never use `sh -c` or build a
  command line from strings.
- **Build paths only in `paths.rs`.** `LOKAL_HOME` has to keep sandboxing everything.
- **Keep `--json` output and `state.toml` backwards compatible.** Adding fields is fine. Renaming or removing one
  needs a new schema version.
- **Don't change the CA's fields** ([SPEC §6.1](docs/SPEC.md#61-root-ca)) unless the change is agreed in an issue
  first. Any change forces every user to re-trust a new CA, which costs them a password prompt.

## Pull requests

- Keep each pull request to one change, with a clear title and description.
- Update the docs your change affects: `README.md`, `docs/SPEC.md`, and `lokal help` text.
- Add a line under `## [Unreleased]` in [`CHANGELOG.md`](CHANGELOG.md) for anything a user would notice.
- Say how you tested it, including the live test summary if it applies.

## The website

The website and user docs (https://mehmetkose.github.io/lokal/) are an [Rspress](https://rspress.rs) site in
[`website/`](website/). You need Node 22.12 or later and pnpm. Run `pnpm install` and `pnpm dev` in that folder;
[`website/README.md`](website/README.md) explains the layout and how docs pages are written. Every push to `main`
that touches `website/` publishes the site to GitHub Pages.

## Releases

Maintainers cut releases by following [`docs/RELEASING.md`](docs/RELEASING.md). A version tag builds the
binaries and drafts the GitHub release, and publishing it updates the Homebrew tap.

## Using a coding agent

lokal was built with coding agents, and they're welcome here. Point yours at this file and
[`docs/SPEC.md`](docs/SPEC.md). An agent must never run `sudo` or change keychain trust on your Mac, and the
rules above apply to its changes too. Review agent-written changes as carefully as your own.

## License

lokal is licensed under the [Apache License, Version 2.0](LICENSE). Unless you explicitly state otherwise, any
contribution you intentionally submit for inclusion in lokal, as defined in the Apache-2.0 license, is licensed
under the same terms, without any additional terms or conditions.
