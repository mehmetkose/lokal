---
title: Upgrading and uninstalling
description: Switch the background service to a new lokal version, and remove lokal cleanly from your Mac.
section: guides
order: 6
sidebarLabel: Upgrade and uninstall
source: [README.md, packaging/homebrew/lokal.rb.in, docs/PACKAGING.md, docs/SPEC.md, docs/SECURITY-MODEL.md, Makefile]
---

Upgrading or removing lokal takes one lokal command on top of your package manager. After an upgrade, run
`lokal upgrade`. Before you remove the package, run `lokal uninstall`. This page explains why, and what each
command keeps and removes.

## Why the extra step

`lokal setup` copies the `lokal` binary to `~/Library/Application Support/lokal/bin/lokal`, and the background
service (the LaunchAgent `dev.lokal.daemon`) runs that copy. That's why the CLI can live anywhere, but it also
means:

- **Updating the package doesn't update the background service.** Until you run `lokal upgrade`, the service keeps
  running the old build, and lokal warns:
  `warning: the background service runs a different lokal build — run 'lokal upgrade'`.
- **Removing the package doesn't undo setup.** The LaunchAgent, the trusted CA, `/etc/resolver/lokal` and the
  service's copy of the binary all stay, and the service keeps running.

## Upgrading

### With Homebrew

```console
$ brew upgrade lokal
$ lokal upgrade
✓ background service now running lokal <version> (<hash>) — ready in 210ms
```

`lokal upgrade` copies the `lokal` binary you ran to the service's location, restarts the service, and waits until
the new build answers. It needs no sudo. Connections that arrive during the restart wait in launchd's queue
instead of failing. Your projects, services and CA stay as they are.

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
`--purge` if you're done with lokal for good. It deletes `~/Library/Application Support/lokal/` and the logs.

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

Drop `--purge` from any of these if you might come back and want your projects restored.

> **Note:** If you already removed the CLI without running `lokal uninstall`, setup is still in place and the
> background service keeps running. Install lokal again, run `lokal uninstall`, then remove it.

## Starting over

To reset lokal completely, with a new CA and no projects:

```console
$ lokal uninstall --purge   # removes everything, including your projects and the CA
$ lokal setup
```

Do the same if you think lokal's CA private key leaked. It untrusts and deletes the old CA and creates a new one.
The [security model](/docs/concepts/security-model) covers what a leaked key could and couldn't do.
