---
title: Installing lokal
description: Install the lokal CLI with Homebrew, a prebuilt binary or from source, then run lokal setup once.
section: start
order: 2
sidebarLabel: Installation
source: [README.md, packaging/homebrew/lokal.rb.in, docs/PACKAGING.md, CHANGELOG.md, Makefile]
---

lokal is a single binary. Install it in one of the three ways below, then run [`lokal setup`](/docs/setup) once
to finish. Installing the CLI changes nothing else on your Mac. Setup is the step that trusts lokal's certificate
and starts the background service.

You need macOS 14 Sonoma or later. Apple Silicon is the tested platform. Intel builds are published, but they
haven't been tested on an Intel Mac yet.

## Homebrew

```console
$ brew install mehmetkose/tap/lokal
$ lokal setup
```

The formula lives in the [mehmetkose/homebrew-tap](https://github.com/mehmetkose/homebrew-tap) repository and
installs the prebuilt binary for your Mac's architecture. It requires macOS Sonoma or later. It doesn't run setup
for you, because setup asks for your password. After the install, Homebrew prints this reminder:

```text
To finish installing lokal, run this once (it asks for your password):
  lokal setup

After each upgrade, switch the background service to the new version:
  lokal upgrade

Before uninstalling, remove lokal's background service, certificate and resolver:
  lokal uninstall          # add --purge to also delete projects and the CA
```

In short: after `brew upgrade lokal`, run `lokal upgrade`. Before `brew uninstall lokal`, run `lokal uninstall`.
[Upgrading and uninstalling](/docs/guides/upgrade-and-uninstall) explains why.

## Prebuilt binary

Each [release](https://github.com/mehmetkose/lokal/releases) has a tarball for Apple Silicon
(`aarch64-apple-darwin`) and one for Intel Macs (`x86_64-apple-darwin`), with SHA-256 checksums. This downloads
one, checks it and installs the CLI to `~/.local/bin`:

```console
$ v=0.3.0; t=aarch64-apple-darwin        # t=x86_64-apple-darwin on an Intel Mac
$ curl -fLO "https://github.com/mehmetkose/lokal/releases/download/v$v/lokal-$v-$t.tar.gz"
$ curl -fLO "https://github.com/mehmetkose/lokal/releases/download/v$v/lokal-$v-$t.tar.gz.sha256"
$ shasum -a 256 -c "lokal-$v-$t.tar.gz.sha256"
$ tar -xzf "lokal-$v-$t.tar.gz"
$ mkdir -p ~/.local/bin && install -m 755 "lokal-$v-$t/lokal" ~/.local/bin/lokal
$ lokal setup
```

Each tarball holds one folder, `lokal-<version>-<target>/`, with the `lokal` binary, `README.md`,
`CHANGELOG.md` and the two license files. Every release also has a `SHA256SUMS` file with the checksums of all its
tarballs.

You can put the binary in any directory on your `PATH`. Nothing depends on where the CLI lives, because setup
gives the background service its own copy.

### Gatekeeper and quarantine

The binaries are ad-hoc signed, but not signed with a Developer ID or notarized by Apple. A tarball downloaded
with `curl` runs as it is. If you download it in a browser instead, macOS quarantines it and Gatekeeper blocks the
binary. Remove the quarantine before you extract the tarball:

```console
$ xattr -d com.apple.quarantine lokal-<version>-<target>.tar.gz
```

Homebrew downloads aren't quarantined.

### Updating a prebuilt binary

Download the new tarball the same way and replace the binary. Then run `lokal upgrade`, so the background service
switches to the new version.

## From source

You need [Rust](https://rustup.rs) 1.89 or later and the Xcode Command Line Tools (`xcode-select --install`).
There are no other system dependencies.

```console
$ git clone https://github.com/mehmetkose/lokal
$ cd lokal
$ make install        # builds and installs the CLI to ~/.local/bin (BINDIR=… to change)
$ lokal setup
```

`make install` builds a release binary and installs it as `~/.local/bin/lokal`. To install it somewhere else,
set `BINDIR`:

```console
$ make install BINDIR="$HOME/bin"
```

If the directory isn't on your `PATH`, `make install` prints a note.

After you pull changes, run `make install` again. Once lokal is set up, it also switches the background service
to the new build (it runs `lokal upgrade` for you). `make uninstall` removes only the CLI. To remove everything,
see [Upgrading and uninstalling](/docs/guides/upgrade-and-uninstall).

## Check the install

```console
$ lokal --version
lokal 0.3.0 (<hash> <build time>)
```

Running `lokal` with no arguments before setup prints a one-line welcome, `Get started: lokal setup`, and the
list of commands.

Next, run [`lokal setup`](/docs/setup). When it finishes, `lokal doctor` checks every layer (setup, the CA and
its trust, the background service, the listeners, a self-test and each service) and says how to fix anything
that's broken:

```console
$ lokal doctor
```
