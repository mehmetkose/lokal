# Packaging lokal

Notes for anyone who packages lokal (a Homebrew formula, a `.deb` or `.rpm`, prebuilt release binaries). lokal's
background service works a little differently from most CLIs, and two of these points affect what users have to
do after an update or an uninstall.

## Building

```console
$ cargo build --release --locked -p lokal
```

- The only artifact is `target/release/lokal`. The `testkit` crate is dev-only and is never shipped.
- Requirements: Rust 1.89 or later (`rust-version` in `Cargo.toml`), and a C compiler, since `ring` builds its C and
  assembly with it:
  - **macOS:** macOS 14 or later and the Xcode Command Line Tools;
  - **Linux:** `build-essential` (Debian, Ubuntu) or `gcc` (Fedora). There's no OpenSSL dependency.
- The release profile strips symbols.
- `build.rs` embeds the git short hash and the build time. `lokal --version` prints
  `lokal 0.1.0 (<hash> <build time>)`. Outside a git checkout, such as a release tarball, the hash is `nogit00`.
  The build time still makes every build's id unique, which `lokal upgrade` relies on.

## The background service runs its own copy

`lokal setup` and `lokal upgrade` copy the running `lokal` binary into lokal's data folder, and the background
service runs that copy:

| | The copy | The service |
|---|---|---|
| macOS | `~/Library/Application Support/lokal/bin/lokal` | the LaunchAgent `dev.lokal.daemon` |
| Linux | `~/.local/share/lokal/bin/lokal` | the systemd user units `dev.lokal.daemon.service` and its three `dev.lokal.daemon-*.socket` units, in `~/.config/systemd/user/` |

So:

- **The CLI can be installed anywhere** (`/opt/homebrew/bin`, `/usr/bin`, `~/.local/bin`). Nothing depends on
  its path.
- **Updating the package doesn't update the background service.** Until the user runs `lokal upgrade`, the
  daemon keeps running the old build, and the CLI warns: `the background service runs a different lokal build —
  run 'lokal upgrade'`.
- `lokal upgrade` exits with code 3 if setup hasn't run yet. An automated hook should run it only when
  `lokal status --json` reports `"setup_done": true`. The repository's `Makefile` does exactly that.

## Setup and uninstall need the user

- **Never run `lokal setup` from an installer.** It asks for the user's password interactively (sudo, and on macOS
  a trust dialog), and it refuses to run as root. Tell the user to run it.
- **Removing the package doesn't undo setup.** Everything setup made stays, and the daemon keeps running:
  - **macOS:** the LaunchAgent, the trusted CA, `/etc/resolver/lokal` and the daemon's copy of the binary;
  - **Linux:** the user units, the CA in the system store (`lokal-<sha1>.crt` among the anchors) and in the
    browsers' NSS databases, the DNS delegate `/etc/systemd/dns-delegate.d/lokal.dns-delegate`,
    `/etc/sysctl.d/50-lokal.conf` (the unprivileged port start, lowered to 80), and the daemon's copy.

  Users should run `lokal uninstall` (or `lokal uninstall --purge`) **before** they remove the package.

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
| `lokal-<version>-aarch64-apple-darwin.tar.gz` | macOS on Apple Silicon, the tested platform |
| `lokal-<version>-x86_64-apple-darwin.tar.gz` | macOS on Intel, cross-compiled. CI only checks that it runs (`--version` under Rosetta) |
| `lokal-<version>-x86_64-unknown-linux-gnu.tar.gz` | Linux on x86_64 (since 0.5.1) |
| `lokal-<version>-aarch64-unknown-linux-gnu.tar.gz` | Linux on arm64 (since 0.5.1) |
| `lokal_<version>_amd64.deb`, `lokal_<version>_arm64.deb` | Linux packages for Debian-based distributions ([below](#linux-packages-deb-and-rpm)) |
| `lokal-<version>-1.x86_64.rpm`, `lokal-<version>-1.aarch64.rpm` | Linux packages for Fedora ([below](#linux-packages-deb-and-rpm)) |
| `<asset>.sha256`, `SHA256SUMS` | SHA-256 checksums in `shasum -a 256` / `sha256sum` format |

- Each tarball holds one folder, `lokal-<version>-<target>/`, with `lokal`, `README.md`, `LICENSE` and
  `CHANGELOG.md`. `scripts/package.sh <target>` builds it into `dist/`. On Linux, the same command builds the two
  packages too. It must run on Linux of the target's own architecture, with `dpkg-deb` and `rpmbuild` installed.
- The macOS binaries are ad-hoc signed (`codesign --sign -`), not signed with a Developer ID or notarized.
  Downloads with `curl` or Homebrew aren't quarantined and run as they are. A browser download is quarantined, and
  Gatekeeper blocks the binary unless the user runs `xattr -d com.apple.quarantine` on the tarball before
  extracting it.
- The Linux binaries are built natively on Ubuntu 24.04 (x86_64 and arm64), so they need **glibc 2.39** or
  later. `scripts/package.sh` fails if a binary needs a newer glibc, or links any library other than `libc`,
  `libm`, `libgcc_s` and the dynamic loader. Every distribution that can run setup (systemd 258 or later) has
  glibc 2.42 or later. Before each release, `scripts/smoke-linux.sh` checks the tarball on Ubuntu 24.04 and
  26.04, Debian 13, Fedora 44 and Arch Linux (x86_64), in containers.

## Linux packages (.deb and .rpm)

Each release has a `.deb` and an `.rpm` for x86_64 and arm64, built by `scripts/package.sh` from the release
binary with the distributions' own tools (`dpkg-deb`, `rpmbuild`). They install:

- `/usr/bin/lokal`;
- `README.md` and `CHANGELOG.md` in `/usr/share/doc/lokal/`, and the license (`/usr/share/doc/lokal/copyright`
  in the `.deb`, `/usr/share/licenses/lokal/LICENSE` in the `.rpm`).

**They have no maintainer scripts.** Setup can't run from one (above), and a package removal can't run each
user's `lokal uninstall`. So the packages work like the Homebrew formula:
- run `lokal setup` once after installing;
- run `lokal upgrade` after each upgrade;
- run `lokal uninstall` before removing the package.

**Dependencies.** lokal runs these programs on Linux, each from an absolute path:

| Program | Used for | `.deb` | `.rpm` |
|---|---|---|---|
| `systemctl`, `busctl` | the user units; systemd-resolved's DNS delegates | Depends: `systemd (>= 258)` | Requires: `systemd >= 258` |
| `ss` | which process holds a port | Depends: `iproute2` | Requires: `iproute` |
| `sysctl` | the port floor | Depends: `procps` | Requires: `procps-ng` |
| `update-ca-certificates` / `update-ca-trust` | the system trust store | Depends: `ca-certificates` | Requires: `ca-certificates` |
| `sudo` | setup and uninstall | Recommends | Recommends |
| `certutil` | Chrome's and Firefox's NSS databases | Recommends: `libnss3-tools` | Recommends: `nss-tools` |
| `xdg-open` | `lokal open` | Recommends: `xdg-utils` | Recommends: `xdg-utils` |
| systemd-resolved, nss-resolve | `.lokal` lookups | Suggests: `systemd-resolved`, `libnss-resolve` | Suggests: `systemd-resolved` |

- **systemd 258** is what setup needs for the `.lokal` DNS delegate. Requiring it makes apt and dnf refuse the
  package where setup couldn't run, such as Ubuntu 24.04 (systemd 255) and Debian 13 (257), instead of installing
  a CLI that can't be set up. Today that means Ubuntu 26.04 and later, Fedora 43 and later, and Debian testing.
- **The `.deb`** also depends on `libc6` at the newest glibc version the binary links, and on `libgcc-s1`.
  `rpmbuild` works out the `.rpm`'s library requirements from the binary itself.
- **systemd-resolved and libnss-resolve are only suggested.** Installing either changes how the whole machine
  resolves names, which a package shouldn't do as a side effect. Ubuntu and Fedora run resolved already. Where it
  isn't running, or isn't on the lookup path (WSL needs `libnss-resolve`), setup says so and stops before it
  changes anything.

The packages aren't signed and aren't in an apt or dnf repository, so `apt upgrade` doesn't update them. To
update, install the newer file the same way.

## Homebrew

The formula lives in [mehmetkose/homebrew-tap](https://github.com/mehmetkose/homebrew-tap):
`brew install mehmetkose/tap/lokal` installs the prebuilt tarball for the machine's OS and architecture, on macOS
and, since 0.5.1, on Linux. It's generated from `packaging/homebrew/lokal.rb.in` by `scripts/homebrew-formula.sh`
each time a release is published, so edit the template, not the tap. It has no `post_install` hook, because setup
needs the user's password, and it shows the message above as its caveats.

- The formula picks its tarball with a plain class-level `if OS.mac? && Hardware::CPU.arm? … elsif … end`.
  `brew audit --strict` doesn't allow `url` inside `on_macos`/`on_linux`/`on_arm`/`on_intel` blocks, and leaves
  class-level OS conditionals to taps. Its macOS requirement sits in `on_macos`, since a top-level
  `depends_on macos:` would rule out Linux.
- On Linux, Homebrew installs the prebuilt binary through its build-from-source path, so it needs a C compiler on
  `PATH` (Homebrew's usual Linux prerequisite, `build-essential` on Debian and Ubuntu).
- Install by the full name, `brew install mehmetkose/tap/lokal`: that also marks the tap as trusted. With only
  `brew tap`, Homebrew 6 and later need `brew trust` first.

## Stable interfaces

Scripts and wrappers can rely on these:

- **Exit codes** ([SPEC §4.9](SPEC.md#49-exit-codes)): 0 success; 1 runtime error, or `doctor` found a failure; 2
  usage error; 3 setup hasn't run; 4 sudo needed but not possible.
- **`--json` output** with `"schema": 1` ([SPEC §4.8](SPEC.md#48-json-output)). Fields may be added, but not
  renamed or removed, without a schema change.
- **`NO_COLOR`** and `--no-color` turn off colour. Colour is also off when stdout isn't a terminal.
