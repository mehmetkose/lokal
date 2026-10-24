# Packaging lokal

Notes for anyone who packages lokal (a Homebrew formula, prebuilt release binaries). lokal's background service
works a little differently from most CLIs, and two of these points affect what users have to do after an update
or an uninstall.

## Building

```console
$ cargo build --release --locked -p lokal
```

- The only artifact is `target/release/lokal`. The `testkit` crate is dev-only and is never shipped.
- Requirements: macOS 14 or later, Rust 1.89 or later (`rust-version` in `Cargo.toml`), and the Xcode Command
  Line Tools. There are no other system dependencies, since `ring` builds its C and assembly with the system
  clang.
- The release profile strips symbols.
- `build.rs` embeds the git short hash and the build time. `lokal --version` prints
  `lokal 0.1.0 (<hash> <build time>)`. Outside a git checkout, such as a release tarball, the hash is `nogit00`.
  The build time still makes every build's id unique, which `lokal upgrade` relies on.

## The background service runs its own copy

`lokal setup` and `lokal upgrade` copy the running `lokal` binary to
`~/Library/Application Support/lokal/bin/lokal`, and the LaunchAgent (`dev.lokal.daemon`) runs that copy. So:

- **The CLI can be installed anywhere** (`/opt/homebrew/bin`, `~/.local/bin`). Nothing depends on its path.
- **Updating the package doesn't update the background service.** Until the user runs `lokal upgrade`, the
  daemon keeps running the old build, and the CLI warns: `the background service runs a different lokal build —
  run 'lokal upgrade'`.
- `lokal upgrade` exits with code 3 if setup hasn't run yet. An automated hook should run it only when
  `lokal status --json` reports `"setup_done": true`. The repository's `Makefile` does exactly that.

## Setup and uninstall need the user

- **Never run `lokal setup` from an installer.** It asks for the user's password interactively (sudo plus a macOS
  trust dialog) and refuses to run as root. Tell the user to run it.
- **Removing the package doesn't undo setup.** The LaunchAgent, the trusted CA, `/etc/resolver/lokal` and the
  daemon's copy of the binary all stay, and the daemon keeps running. Users should run `lokal uninstall` (or
  `lokal uninstall --purge`) **before** they remove the package.

Suggested post-install message (for example, Homebrew `caveats`):

```text
To finish installing lokal, run this once in your Terminal (it asks for your password):
  lokal setup

After each upgrade, switch the background service to the new version:
  lokal upgrade

Before uninstalling, remove lokal's background service, certificate and resolver:
  lokal uninstall          # add --purge to also delete projects and the CA
```

## Prebuilt binaries

The project publishes its own ([`RELEASING.md`](RELEASING.md)). Each GitHub release has:

| Asset | What it is |
|---|---|
| `lokal-<version>-aarch64-apple-darwin.tar.gz` | Apple Silicon, the tested platform |
| `lokal-<version>-x86_64-apple-darwin.tar.gz` | Intel, cross-compiled. CI only checks that it runs (`--version` under Rosetta) |
| `<tarball>.sha256`, `SHA256SUMS` | SHA-256 checksums in `shasum -a 256` format |

- Each tarball holds one folder, `lokal-<version>-<target>/`, with `lokal`, `README.md`, `LICENSE` and
  `CHANGELOG.md`. `scripts/package.sh <target>` builds it into `dist/`.
- The binaries are ad-hoc signed (`codesign --sign -`), not signed with a Developer ID or notarized. Downloads
  with `curl` or Homebrew aren't quarantined and run as they are. A browser download is quarantined, and
  Gatekeeper blocks the binary unless the user runs `xattr -d com.apple.quarantine` on the tarball before
  extracting it.

## Homebrew

The formula lives in [mehmetkose/homebrew-tap](https://github.com/mehmetkose/homebrew-tap):
`brew install mehmetkose/tap/lokal` installs the prebuilt tarball for the Mac's architecture. It's generated
from `packaging/homebrew/lokal.rb.in` by `scripts/homebrew-formula.sh` each time a release is published, so edit
the template, not the tap. It has no `post_install` hook, because setup needs the user's password, and it shows
the message above as its caveats.

## Stable interfaces

Scripts and wrappers can rely on these:

- **Exit codes** ([SPEC §4.9](SPEC.md#49-exit-codes)): 0 success; 1 runtime error, or `doctor` found a failure; 2
  usage error; 3 setup hasn't run; 4 sudo needed but not possible.
- **`--json` output** with `"schema": 1` ([SPEC §4.8](SPEC.md#48-json-output)). Fields may be added, but not
  renamed or removed, without a schema change.
- **`NO_COLOR`** and `--no-color` turn off colour. Colour is also off when stdout isn't a terminal.
