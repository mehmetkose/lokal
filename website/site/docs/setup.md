---
title: Setting up lokal
description: What lokal setup changes on your Mac or Linux machine, the only commands it runs with sudo, and how to repair it.
section: start
order: 3
sidebarLabel: Setup
source: [README.md, docs/SECURITY-MODEL.md, docs/SPEC.md, docs/TROUBLESHOOTING.md, docs/PORTING.md, crates/lokal/src/setup/mod.rs, crates/lokal/src/platform/linux/trust.rs, crates/lokal/src/platform/linux/nss.rs, crates/lokal/src/platform/linux/resolved.rs, crates/lokal/src/platform/linux/systemd.rs]
---

`lokal setup` runs once per Mac. It's the only part of installing lokal that changes system settings: it trusts
lokal's certificate, sends `.lokal` lookups to lokal and starts the background service. It shows its plan and asks before it changes
anything, and it takes about 10 seconds.

This page describes macOS. On Linux (a preview), setup takes the same steps with Linux's own tools: see
[On Linux](#on-linux).

Setup and `lokal uninstall` are the only commands that ever ask for your password. Creating projects and adding,
changing or removing services never do.

```console
$ lokal setup
```

Run it as yourself, not with `sudo`. lokal asks for your password itself, for the steps that need it, and refuses
to run setup as root.

## What setup does

On a fresh Mac, setup:

1. **Creates lokal's certificate authority** in `~/Library/Application Support/lokal/ca/`, as you. It can only
   sign names under `.lokal` and `.local`, and its private key never leaves that folder.
2. **Trusts it in the macOS System keychain**, so browsers and macOS tools accept lokal's certificates.
3. **Sends `.lokal` lookups to lokal** by installing `/etc/resolver/lokal`. This file tells macOS to ask lokal's
   built-in DNS server (`127.0.0.1:53535`) about `*.lokal` names.
4. **Installs the background service**, a LaunchAgent (`dev.lokal.daemon`) that runs as you. launchd holds ports
   443 and 80 on `127.0.0.1` and `::1` for it, so it never needs root, and nothing is reachable from your network.
5. **Runs a self-test.** It publishes a throwaway name for each TLD, resolves it, and fetches it over HTTPS
   verified by macOS.

If Firefox is installed, setup also configures it to trust lokal (see [Firefox](#firefox) below). Setup also
writes the certificate bundle that [`lokal env`](/docs/guides/lokal-env) uses. When it finishes, it prints the
next commands to run:

```text
You're all set. Next:
  lokal project create myproject
  lokal service add api 4000
Tip: run 'eval "$(lokal env)"' so Node and Python trust lokal too.
```

## The password prompts

Steps 2 and 3 are the only ones that need your password. Setup prints these commands before it runs them, and
they are the only commands it runs with sudo (`/Users/you` stands for your home folder):

```text
sudo /usr/bin/security add-trusted-cert -d -r trustRoot -p ssl -k /Library/Keychains/System.keychain "/Users/you/Library/Application Support/lokal/ca/root.pem"
sudo /bin/mkdir -p /etc/resolver
sudo /usr/bin/install -m 0644 "/Users/you/Library/Application Support/lokal/resolver/lokal" /etc/resolver/lokal
```

lokal runs each one through `sudo` with an argument vector. It never runs a shell and never builds a command line
from strings. lokal's only other privileged commands undo these: `lokal uninstall` runs them, and so does setup
when it [replaces an outdated CA](#running-setup-again).

You type your password once in Terminal for sudo. macOS then shows its own dialog and asks for it again, because
it confirms every change to certificate trust settings separately. On Linux there's no second dialog: sudo asks
once.

After setup, nothing runs as root. lokal asks for your password again only in `lokal uninstall`, or in
`lokal setup` if it has to repair one of these two steps. The [security model](/docs/concepts/security-model)
lists every file and setting setup changes, and what undoes each one.

## Preview the plan with `--dry-run`

`lokal setup --dry-run` prints the plan and the exact privileged commands, then exits without changing anything.
On a fresh Mac it looks like this:

```console
$ lokal setup --dry-run
Welcome to lokal. This one-time setup makes https://<anything>.lokal (and .local) work on this Mac.

  1. Create lokal's certificate authority
     Only valid for .lokal and .local names. Its key stays in your user folder.
  2. Trust it in the macOS System keychain                        ← needs your password (once)
  3. Send .lokal lookups to lokal (/etc/resolver/lokal)          ← same password
     You type your password once in Terminal (sudo); macOS then shows its own dialog for the keychain.
  4. Install the background service (LaunchAgent, runs as you)
     macOS will hold ports 443 and 80 (and DNS on 53535) on 127.0.0.1 for it. Nothing is reachable from your network.
  5. Run a self-test

Privileged commands (the only ones; run with sudo):
  sudo /usr/bin/security add-trusted-cert -d -r trustRoot -p ssl -k /Library/Keychains/System.keychain "/Users/you/Library/Application Support/lokal/ca/root.pem"
  sudo /bin/mkdir -p /etc/resolver
  sudo /usr/bin/install -m 0644 "/Users/you/Library/Application Support/lokal/resolver/lokal" /etc/resolver/lokal

Dry run: nothing was changed.
```

Steps that are already in place are marked `(already done)`. If nothing privileged is missing, the dry run says
`No privileged command is needed.`

## Skip the confirmation with `--yes`

Setup asks `Continue? [Y/n]` before it changes anything. `lokal setup --yes` (or `-y`) skips that question. It
doesn't skip the password prompts: sudo still needs a terminal to ask for your password.

If setup needs sudo but can't ask for a password, it stops before it changes anything and exits with code 4. That
happens when no terminal is attached, for example when a script or a coding agent runs it. Run `lokal setup`
yourself in a Terminal window. A coding agent should never run setup. It should ask you to.

## Running setup again

Setup is safe to re-run, and running it again is the repair path. It checks each step, fixes whatever is missing,
and asks for your password only if one of the privileged steps is missing. If everything is already in place, it
says so and changes nothing:

```text
Everything was already set up — nothing to change.
```

If `lokal doctor` reports a problem with the CA's trust or the `.lokal` resolver, `lokal setup` is the fix. If the
existing CA doesn't have exactly the name constraints this version expects (`lokal doctor` warns about this as
`ca.scope`), setup replaces it. The old CA is untrusted and removed in the same password prompt.

## Firefox

Firefox keeps its own certificate store. If Firefox is installed, setup adds this line to `user.js` in each
Firefox profile:

```js
user_pref("security.enterprise_roots.enabled", true); // added by lokal
```

That preference makes Firefox trust every root CA that a user or administrator added to the macOS keychain, not
only lokal's. If you don't want that, delete the line. lokal then won't work in Firefox, but it keeps working
everywhere else. `lokal uninstall` removes exactly that line.

If you create a Firefox profile after setup, run `lokal setup` again to configure it.

On Linux, setup doesn't use this preference. It adds lokal's CA to Firefox's and Chrome's own certificate
databases instead: see [Browsers on Linux](#browsers-on-linux).

## If setup fails

Start with `lokal doctor`. It checks every layer and says how to fix what's broken. If the self-test fails, setup
names the layer that failed (DNS, connect, TLS and so on) and prints a hint. The background service's log usually
has the details:

```console
$ lokal doctor
$ lokal logs -n 200
```

Common setup problems:

- **`run 'lokal setup' as yourself, not with sudo`.** Run it without the `sudo` prefix.
- **Setup asks for your password twice.** That's expected on macOS: once for sudo in Terminal, once in the macOS
  dialog. On Linux it asks once.
- **Setup fails with exit code 4.** No terminal was attached, or `LOKAL_FORBID_SUDO=1` is set. Run `lokal setup`
  yourself in Terminal.
- **Port 443 or 80 is already in use.** Another program holds the port. `lokal doctor` names it in the
  `listeners` check. Stop that program or move it to another port, then run `lokal restart`.

[Troubleshooting](/docs/troubleshooting) covers these and more.

## On Linux

Linux is a preview. Setup there needs systemd 258 or later with systemd-resolved running, such as Ubuntu 26.04 or
Fedora 43; [Linux (preview)](/docs/installation#linux-preview) lists everything it needs. Only `.lokal` names work
on Linux, so the self-test checks a `.lokal` name only.

### What setup does on Linux

Setup creates the CA the same way, as you, in `~/.local/share/lokal/ca/`. The other steps use Linux's own tools:

- **Trust.** Setup copies the CA into the system certificate store as `lokal-<sha1>.crt` and runs the
  distribution's update tool, which rebuilds the certificate bundle that curl, OpenSSL and many other programs
  read. On Debian and Ubuntu that's `/usr/local/share/ca-certificates` and `update-ca-certificates`; on Fedora
  and Arch, the p11-kit anchors folder and `update-ca-trust`. Setup picks by which tool is installed.
- **Browsers.** Chrome, Chromium and Firefox don't read the system store, so setup also adds the CA to their own
  certificate databases, as you. See [Browsers on Linux](#browsers-on-linux).
- **`.lokal` lookups.** Setup installs a systemd-resolved DNS delegate,
  `/etc/systemd/dns-delegate.d/lokal.dns-delegate`, which sends every `*.lokal` lookup to lokal's DNS server on
  `127.0.0.1:53535`. Then it restarts systemd-resolved, which reads delegates when it starts.
- **The background service.** Setup installs systemd user units in `~/.config/systemd/user/`:
  `dev.lokal.daemon.service`, which runs lokal as you, and three socket units that hold ports 443 and 80 on
  `127.0.0.1` and `::1`, and DNS on `127.0.0.1:53535`, for it. Nothing is reachable from your network.
- **Ports 443 and 80.** A user's service can't bind ports below `net.ipv4.ip_unprivileged_port_start`, which is
  1024 by default. Setup lowers it to 80, with `/etc/sysctl.d/50-lokal.conf` for later boots and `sysctl -w` for
  now. This lets every local user bind ports 80 to 1023, not only lokal. If the setting is already 80 or lower,
  setup skips this step. `lokal uninstall` removes the file and sets the kernel's default, 1024, back.

### The password prompt on Linux

The trust, resolver and port steps need root. Setup runs them as one batch, so you type your password once, for
sudo, and there's no second dialog. It prints the commands before it runs them. On a fresh Ubuntu 26.04 machine,
`lokal setup --dry-run` shows (`/home/you` stands for your home folder):

```console
$ lokal setup --dry-run
Welcome to lokal. This one-time setup makes https://<anything>.lokal work on this computer.

  1. Create lokal's certificate authority
     Only valid for .lokal and .local names. Its key stays in your user folder.
  2. Trust it in the system certificate store                     ← needs your password (once)
  3. Send .lokal lookups to lokal (/etc/systemd/dns-delegate.d/lokal.dns-delegate)          ← same password
     You type your password once (sudo).
  4. Install the background service (systemd user units, runs as you)
     systemd will hold ports 443 and 80 (and DNS on 53535) on 127.0.0.1 for it. Nothing is reachable from your network.
     Ports below 1024 need one system setting: user services may bind ports 80 and up ← same password
  5. Run a self-test

Privileged commands (the only ones; run with sudo):
  sudo /usr/bin/install -m 0644 /home/you/.local/share/lokal/ca/root.pem /usr/local/share/ca-certificates/lokal-<sha1>.crt
  sudo /usr/sbin/update-ca-certificates
  sudo /usr/bin/install -D -m 0644 /home/you/.local/share/lokal/resolver/lokal /etc/systemd/dns-delegate.d/lokal.dns-delegate
  sudo /usr/bin/systemctl restart systemd-resolved
  sudo /usr/bin/install -m 0644 /home/you/.local/share/lokal/resolver/ports.conf /etc/sysctl.d/50-lokal.conf
  sudo /usr/sbin/sysctl -w net.ipv4.ip_unprivileged_port_start=80

Dry run: nothing was changed.
```

On Fedora, the first two commands install the CA into `/etc/pki/ca-trust/source/anchors/` and run
`update-ca-trust` instead. The [security model](/docs/concepts/security-model#on-linux) lists every file these
commands change, and what undoes each one.

### Browsers on Linux

Chrome, Chromium and Firefox on Linux trust only their own certificate databases (NSS). After the password step,
setup adds lokal's CA to each one it finds, with `certutil`, as you and never as root: `~/.pki/nssdb` for Chrome
and Chromium, snap Chromium's database, and every Firefox profile, including the snap and Flatpak ones. The CA is
trusted there for HTTPS servers only. If Chrome is installed but hasn't run yet, setup creates its database. Each
database gets a line:

```text
  ✓ Browser certificates ~/.pki/nssdb: trusted lokal's CA
```

`certutil` comes from `libnss3-tools` on Debian and Ubuntu, and `nss-tools` on Fedora. The `.deb` and `.rpm`
recommend it. Without it, setup still finishes, but if Chrome, Chromium or Firefox is installed, it warns:

```text
  ! Browsers: certutil is missing, so browsers don't trust lokal yet: install libnss3-tools (Debian, Ubuntu) or nss-tools (Fedora), then run 'lokal setup' again
```

If you create a Firefox profile after setup, run `lokal setup` again. `lokal doctor` checks every database (in its
`firefox` check), and `lokal uninstall` removes lokal's CA from each one.

### When setup stops on Linux

Before it changes anything, setup checks that each step can work on this machine. If one can't, it stops with a
message that starts `lokal can't be set up here yet:`, and nothing is changed. `lokal setup --dry-run` prints the
plan, then the same message, and exits 1. The reasons:

- **`systemd-resolved isn't running, and lokal's .lokal names need it (systemd 258 or later)`.** lokal sends
  `.lokal` lookups through systemd-resolved. Ubuntu and Fedora run it by default.
- **`this systemd-resolved has no DNS delegates, which lokal's .lokal names need (systemd 258 or later)`.** The
  machine's systemd is older than 258, such as Ubuntu 24.04 (255) or Debian 13 (257). Setup can't run there yet.
- **`programs here don't ask systemd-resolved for names (…)`.** resolved runs, but programs look names up
  elsewhere: the hosts line of `/etc/nsswitch.conf` has no `resolve`, and `/etc/resolv.conf` doesn't point at
  resolved's stub. The message ends with the fix: on Debian and Ubuntu, `sudo apt install libnss-resolve`. WSL
  writes its own `resolv.conf`, so under WSL you need it.
- **`no system certificate store: install the ca-certificates package`.** Neither `update-ca-certificates` nor
  `update-ca-trust` is installed. Install `ca-certificates`.

After the fix, run `lokal setup` again. [Troubleshooting](/docs/troubleshooting#on-linux) covers problems after
setup on Linux.

## Next

Setup done? Follow the [quick start](/docs/quick-start) to create a project and add your first service.
