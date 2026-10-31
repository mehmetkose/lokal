---
title: Upgrading and uninstalling
description: Switch the background service to a new lokal version, and remove lokal cleanly from your Mac or Linux machine.
section: guides
order: 7
sidebarLabel: Upgrade and uninstall
source: [README.md, packaging/homebrew/lokal.rb.in, docs/PACKAGING.md, docs/PORTING.md, docs/SPEC.md, docs/SECURITY-MODEL.md, Makefile, crates/lokal/src/setup/mod.rs, crates/lokal/src/platform/linux/systemd.rs]
---

Upgrading or removing lokal takes one lokal command on top of your package manager. After an upgrade, run
`lokal upgrade`. Before you remove the package, run `lokal uninstall`. This page explains why, and what each
command keeps and removes.

## Why the extra step

`lokal setup` copies the `lokal` binary to `~/Library/Application Support/lokal/bin/lokal`, and the background
service (the LaunchAgent `dev.lokal.daemon`) runs that copy. On Linux, the copy is
`~/.local/share/lokal/bin/lokal`, and systemd user units run it ([On Linux](#on-linux)). That's why the CLI can live
anywhere, but it also means:

- **Updating the package doesn't update the background service.** Until you run `lokal upgrade`, the service keeps
  running the old build, and lokal warns:
  `warning: the background service runs a different lokal build — run 'lokal upgrade'`.
- **Removing the package doesn't undo setup.** The LaunchAgent, the trusted CA, `/etc/resolver/lokal` and the
  service's copy of the binary all stay, and the service keeps running. On Linux, the same goes for the units, the
  CA in the system store and your browsers, the DNS delegate and the port setting.

## Upgrading

### With Homebrew

```console
$ brew upgrade lokal
$ lokal upgrade
✓ background service now running lokal <version> (<hash>) — ready in 210ms
```

`lokal upgrade` copies the `lokal` binary you ran to the service's location, restarts the service, and waits until
the new build answers. It needs no sudo. Connections that arrive during the restart wait in the socket's queue
(launchd's, or systemd's on Linux) instead of failing. Your projects, services and CA stay as they are.

The same formula works with Homebrew on Linux. To upgrade a `.deb` or `.rpm`, see
[On Linux](#upgrading-a-deb-or-rpm).

### With a prebuilt binary

Download the new tarball and replace the binary (see [Prebuilt binary](/docs/installation#prebuilt-binary)), then
run `lokal upgrade`.

### From source

```console
$ git pull
$ make install        # rebuilds and installs the CLI
```

Once lokal is set up, `make install` also runs `lokal upgrade` for you.

### Check that the upgrade took

`lokal status` shows the build the background service runs, and warns if it differs from the CLI's. `lokal doctor`
checks the same thing in its `daemon.build` check.

`lokal upgrade` exits with code 3 if lokal isn't set up yet. In that case, run [`lokal setup`](/docs/setup)
instead. It installs the background service from the current binary.

## Uninstalling

Run `lokal uninstall` **before** you remove the CLI, while the `lokal` command is still installed:

```console
$ lokal uninstall            # removes the service, the resolver file and the CA's trust (asks for your password)
$ lokal uninstall --purge    # also deletes your projects, the CA files and the logs
```

`lokal uninstall` shows its plan and asks `Continue? [y/N]` before it changes anything. `--yes` (or `-y`) skips the
question. It then:

1. stops the background service and removes the LaunchAgent and its copy of lokal;
2. untrusts and removes every lokal CA from the System keychain;
3. removes `/etc/resolver/lokal`;
4. removes the Firefox setting, if setup added one.

Steps 2 and 3 need your password: once in Terminal for sudo, and macOS may show its own dialog asking for it
again. Uninstall prints those commands before it runs them. If some pieces are already gone, it still succeeds.
On Linux, uninstall removes Linux's pieces instead: see [Uninstalling on Linux](#uninstalling-on-linux).

### Plain or `--purge`

| | `lokal uninstall` | `lokal uninstall --purge` |
|---|---|---|
| Background service and its copy of lokal | Removed | Removed |
| CA trust in the System keychain | Removed | Removed |
| `/etc/resolver/lokal` | Removed | Removed |
| Firefox setting | Removed | Removed |
| Projects and services (`state.toml`) | Kept | Deleted |
| CA files, including the private key | Kept | Deleted |
| Logs (`~/Library/Logs/lokal/`) | Kept | Deleted |

A plain uninstall keeps `state.toml` and the CA files, so running `lokal setup` later restores everything as it
was, without creating a new CA. Once the CA is no longer trusted, the key on disk can't make anything trusted. Use
`--purge` if you're done with lokal for good. It deletes `~/Library/Application Support/lokal/` and the logs
(on Linux, `~/.local/share/lokal/` and `~/.local/state/lokal/`).

Either way, `lokal env` exports nothing after an uninstall. If your shell profile runs `eval "$(lokal env)"` (or
`lokal env --shell fish | source`), remove that line. Uninstall reminds you.

### Then remove the CLI

With Homebrew:

```console
$ lokal uninstall --purge
$ brew uninstall lokal
```

With a prebuilt binary, delete the file you installed:

```console
$ lokal uninstall --purge
$ rm ~/.local/bin/lokal
```

From source, `make uninstall` removes only the CLI from `~/.local/bin` (pass the same `BINDIR` you installed
with):

```console
$ lokal uninstall --purge
$ make uninstall
```

With a `.deb` or `.rpm` on Linux, remove the package:

```console
$ lokal uninstall --purge
$ sudo apt remove lokal        # Ubuntu; on Fedora: sudo dnf remove lokal
```

Drop `--purge` from any of these if you might come back and want your projects restored.

> **Note:** If you already removed the CLI without running `lokal uninstall`, setup is still in place and the
> background service keeps running. Install lokal again, run `lokal uninstall`, then remove it.

## On Linux

lokal on Linux is a [preview](/docs/installation#linux-preview). Upgrading and uninstalling work as above, with
Linux's pieces in place of macOS's:

- **The background service** is a set of systemd user units in `~/.config/systemd/user/`: the service,
  `dev.lokal.daemon.service`, and three socket units that hold its ports, `dev.lokal.daemon-https.socket`,
  `dev.lokal.daemon-http.socket` and `dev.lokal.daemon-dns.socket`. The service runs lokal's own copy,
  `~/.local/share/lokal/bin/lokal`.
- **lokal's data** is in `~/.local/share/lokal/`, and its logs are in `~/.local/state/lokal/`. If you set
  `XDG_DATA_HOME`, `XDG_STATE_HOME` or `XDG_CONFIG_HOME`, lokal uses those folders instead.

### Upgrading a `.deb` or `.rpm`

The packages aren't in an apt or dnf repository, so `apt upgrade` and `dnf upgrade` don't update them. Download
the newer file as in [Linux (preview)](/docs/installation#linux-preview), install it the same way, then switch the
background service over:

```console
$ sudo apt install ./lokal_<version>_amd64.deb       # Ubuntu; lokal_<version>_arm64.deb on arm64
$ sudo dnf install ./lokal-<version>-1.x86_64.rpm    # Fedora; lokal-<version>-1.aarch64.rpm on arm64
$ lokal upgrade
```

The package doesn't run `lokal upgrade` for you. With Homebrew, a tarball or a source build, upgrade as on macOS
([above](#upgrading)).

`lokal upgrade` restarts `dev.lokal.daemon.service`. Its socket units keep holding the ports meanwhile, so
connections wait instead of failing, as they do with launchd.

### Uninstalling on Linux

Run `lokal uninstall` before you remove the package, as on macOS. It shows its plan, asks `Continue? [y/N]`, and
then:

1. stops and removes the background service: the systemd user units and the service's copy of lokal. If setup
   lowered the port limit, it also sets the lowest port user services may bind back to 1024: it deletes
   `/etc/sysctl.d/50-lokal.conf` and sets the kernel's default again;
2. untrusts and removes lokal's CA from the system certificate store: it deletes `lokal-<sha1>.crt` from the
   store's anchors and rebuilds the system bundle;
3. removes the DNS delegate, `/etc/systemd/dns-delegate.d/lokal.dns-delegate`, and restarts systemd-resolved;
4. removes lokal's CA from your browsers' certificate databases (Chrome's, and each Firefox profile's), with
   `certutil`, as you.

The port setting and steps 2 and 3 need your password, once, for sudo. There's no second dialog as on macOS.
Uninstall prints the commands before it runs them. On Ubuntu they are:

```text
sudo /usr/bin/rm -f /usr/local/share/ca-certificates/lokal-<sha1>.crt
sudo /usr/sbin/update-ca-certificates
sudo /usr/bin/rm -f /etc/systemd/dns-delegate.d/lokal.dns-delegate
sudo /usr/bin/systemctl restart systemd-resolved
sudo /usr/bin/rm -f /etc/sysctl.d/50-lokal.conf
sudo /usr/sbin/sysctl -w net.ipv4.ip_unprivileged_port_start=1024
```

On Fedora, the CA's file is in `/etc/pki/ca-trust/source/anchors/`, and `update-ca-trust` rebuilds the bundle. If
some pieces are already gone, uninstall skips them and still succeeds.

| | `lokal uninstall` | `lokal uninstall --purge` |
|---|---|---|
| Background service: the systemd user units and `~/.local/share/lokal/bin/lokal` | Removed | Removed |
| lokal's CA in the system certificate store | Removed | Removed |
| lokal's CA in your browsers' certificate databases | Removed | Removed |
| `/etc/systemd/dns-delegate.d/lokal.dns-delegate` | Removed | Removed |
| `/etc/sysctl.d/50-lokal.conf`, and the port limit back at 1024 | Removed | Removed |
| Projects and services (`state.toml`) | Kept | Deleted |
| CA files, including the private key | Kept | Deleted |
| Logs (`~/.local/state/lokal/`) | Kept | Deleted |

Then remove the package with `sudo apt remove lokal` or `sudo dnf remove lokal`
([above](#then-remove-the-cli)). Removing it first doesn't undo setup: install it again, run `lokal uninstall`,
then remove it.

## Starting over

To reset lokal completely, with a new CA and no projects:

```console
$ lokal uninstall --purge   # removes everything, including your projects and the CA
$ lokal setup
```

Do the same if you think lokal's CA private key leaked. It untrusts and deletes the old CA and creates a new one.
The [security model](/docs/concepts/security-model) covers what a leaked key could and couldn't do.
