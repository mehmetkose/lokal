---
title: Setting up lokal
description: What lokal setup changes on your Mac, the only commands it runs with sudo, and how to repair it.
section: start
order: 3
sidebarLabel: Setup
source: [README.md, docs/SECURITY-MODEL.md, docs/SPEC.md, docs/TROUBLESHOOTING.md, crates/lokal/src/setup/mod.rs]
---

`lokal setup` runs once per Mac. It's the only part of installing lokal that changes system settings: it trusts
lokal's certificate, sends `.lokal` lookups to lokal and starts the background service. It shows its plan and asks before it changes
anything, and it takes about 10 seconds.

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
it confirms every change to certificate trust settings separately.

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
- **Setup asks for your password twice.** That's expected: once for sudo in Terminal, once in the macOS dialog.
- **Setup fails with exit code 4.** No terminal was attached, or `LOKAL_FORBID_SUDO=1` is set. Run `lokal setup`
  yourself in Terminal.
- **Port 443 or 80 is already in use.** Another program holds the port. `lokal doctor` names it in the
  `listeners` check. Stop that program or move it to another port, then run `lokal restart`.

[Troubleshooting](/docs/troubleshooting) covers these and more.

## Next

Setup done? Follow the [quick start](/docs/quick-start) to create a project and add your first service.
