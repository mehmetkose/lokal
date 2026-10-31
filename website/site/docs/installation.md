---
title: Installing lokal
description: Install the lokal CLI with Homebrew, a prebuilt binary, a .deb or .rpm on Linux, or from source, then run lokal setup once.
section: start
order: 2
sidebarLabel: Installation
source: [README.md, packaging/homebrew/lokal.rb.in, docs/PACKAGING.md, docs/PORTING.md, CHANGELOG.md, Makefile]
---

lokal is a single binary. Install it in one of the three ways below, then run [`lokal setup`](/docs/setup) once
to finish. Installing the CLI changes nothing else on your Mac. Setup is the step that trusts lokal's certificate
and starts the background service.

You need macOS 14 Sonoma or later. Apple Silicon is the tested platform. Intel builds are published, but they
haven't been tested on an Intel Mac yet. On Linux, lokal is a preview, with `.deb` and `.rpm` packages: see
[Linux (preview)](#linux-preview).

## Homebrew

```console
$ brew install mehmetkose/tap/lokal
$ lokal setup
```

The formula lives in the [mehmetkose/homebrew-tap](https://github.com/mehmetkose/homebrew-tap) repository and
installs the prebuilt binary for your machine. On macOS it requires Sonoma or later. Since 0.5.1 it works on Linux
too, where Homebrew needs a C compiler (`build-essential` on Debian and Ubuntu). It doesn't run setup for you,
because setup asks for your password. After the install, Homebrew prints this reminder:

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
(`aarch64-apple-darwin`) and one for Intel Macs (`x86_64-apple-darwin`), with SHA-256 checksums. (The Linux
tarballs and packages are under [Linux (preview)](#linux-preview).) This downloads one, checks it and installs the
CLI to `~/.local/bin`:

```console
$ v=0.5.0; t=aarch64-apple-darwin        # t=x86_64-apple-darwin on an Intel Mac
$ curl -fLO "https://github.com/mehmetkose/lokal/releases/download/v$v/lokal-$v-$t.tar.gz"
$ curl -fLO "https://github.com/mehmetkose/lokal/releases/download/v$v/lokal-$v-$t.tar.gz.sha256"
$ shasum -a 256 -c "lokal-$v-$t.tar.gz.sha256"
$ tar -xzf "lokal-$v-$t.tar.gz"
$ mkdir -p ~/.local/bin && install -m 755 "lokal-$v-$t/lokal" ~/.local/bin/lokal
$ lokal setup
```

Each tarball holds one folder, `lokal-<version>-<target>/`, with the `lokal` binary, `README.md`,
`CHANGELOG.md` and `LICENSE`. Every release also has a `SHA256SUMS` file with the checksums of all its tarballs.

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
There are no other system dependencies. On Linux, you need Rust and a C compiler (`build-essential` on Debian
and Ubuntu) instead of the Xcode tools.

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

## Linux (preview)

Since 0.5.0, lokal runs on Linux, x86_64 and arm64. Since 0.5.1, each
[release](https://github.com/mehmetkose/lokal/releases) has a `.deb`, an `.rpm` and a tarball for both
architectures, with SHA-256 checksums.

### Ubuntu

The `.deb` needs Ubuntu 26.04 or later:

```console
$ v=0.5.0; a=amd64                       # a=arm64 on arm64
$ curl -fLO "https://github.com/mehmetkose/lokal/releases/download/v$v/lokal_${v}_$a.deb"
$ curl -fLO "https://github.com/mehmetkose/lokal/releases/download/v$v/lokal_${v}_$a.deb.sha256"
$ sha256sum -c "lokal_${v}_$a.deb.sha256"
$ sudo apt install "./lokal_${v}_$a.deb"
$ lokal setup
```

### Fedora

The `.rpm` needs Fedora 43 or later:

```console
$ v=0.5.0; a=x86_64                      # a=aarch64 on arm64
$ curl -fLO "https://github.com/mehmetkose/lokal/releases/download/v$v/lokal-$v-1.$a.rpm"
$ curl -fLO "https://github.com/mehmetkose/lokal/releases/download/v$v/lokal-$v-1.$a.rpm.sha256"
$ sha256sum -c "lokal-$v-1.$a.rpm.sha256"
$ sudo dnf install "./lokal-$v-1.$a.rpm"
$ lokal setup
```

### Other distributions

Use the tarball as in [Prebuilt binary](#prebuilt-binary), with `t=x86_64-unknown-linux-gnu` (or
`aarch64-unknown-linux-gnu`) and `sha256sum -c` in place of `shasum -a 256 -c`. It needs glibc 2.39 or later.
[Homebrew](#homebrew) and [building from source](#from-source) work too.

### What the packages do

They install the `lokal` command and its docs, and the programs lokal runs: `systemd`, `iproute2`, `procps` and
`ca-certificates`, plus `certutil` (`libnss3-tools`, `nss-tools`) as a recommended package. They require systemd
258 or later, so apt and dnf refuse them where setup couldn't run, such as Ubuntu 24.04 (systemd 255) and
Debian 13 (257).

They don't run setup, and they don't update the background service, which runs its own copy of lokal. After you
install a newer package, run `lokal upgrade`. Before `sudo apt remove lokal` or `sudo dnf remove lokal`, run
`lokal uninstall`: removing the package doesn't undo setup. [Upgrading and
uninstalling](/docs/guides/upgrade-and-uninstall) explains why.

### What setup needs

- **systemd 258 or later, with systemd-resolved running**, such as Ubuntu 26.04 or Fedora 43. lokal sends `.lokal`
  lookups to itself through a resolved DNS delegate, `/etc/systemd/dns-delegate.d/lokal.dns-delegate`.
- **Programs that ask systemd-resolved:** `resolve` on the hosts line of `/etc/nsswitch.conf`, or resolved's stub
  as the only server in `/etc/resolv.conf`. WSL writes its own `resolv.conf`, so there, install `libnss-resolve`.
- **A system trust tool:** `update-ca-certificates` (Debian and Ubuntu) or `update-ca-trust` (Fedora).
- **`certutil`** (`libnss3-tools` on Debian and Ubuntu, `nss-tools` on Fedora), to add lokal's CA to Chrome's and
  Firefox's certificate databases. Without it, setup says what to install.

If the trust tool or systemd-resolved can't do its part, setup says why and stops before it changes anything.
It asks for your password once (sudo): to trust the CA, to install the DNS delegate, and to let user services
bind ports 80 and up (`/etc/sysctl.d/50-lokal.conf` sets `net.ipv4.ip_unprivileged_port_start` to 80), so the
background service, a set of systemd user units, can hold ports 443 and 80. That setting lets every local user
bind ports 80–1023; `lokal uninstall` sets the default, 1024, back.

Only `.lokal` names work on Linux; `.local` names don't yet. Setup has been tested on Ubuntu 26.04 under WSL2.

## Check the install

```console
$ lokal --version
lokal 0.5.0 (<hash> <build time>)
```

Running `lokal` with no arguments before setup prints a one-line welcome, `Get started: lokal setup`, and the
list of commands.

Next, run [`lokal setup`](/docs/setup). When it finishes, `lokal doctor` checks every layer (setup, the CA and
its trust, the background service, the listeners, a self-test and each service) and says how to fix anything
that's broken:

```console
$ lokal doctor
```
