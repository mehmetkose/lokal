---
title: Trusting lokal from Node, Python and curl
description: Point tools that ship their own certificate bundle at lokal's CA with one line in your shell.
section: guides
order: 3
sidebarLabel: Node, Python & curl
source: [README.md, docs/SPEC.md, docs/TROUBLESHOOTING.md, docs/SECURITY-MODEL.md, crates/lokal/src/cli/system.rs]
---

Browsers and most macOS tools check certificates against the macOS keychain, so they trust lokal as soon as
[setup](/docs/setup) is done. Node, Python and some other tools ship their own certificate bundles instead, and
reject lokal's certificates until you point them at lokal's CA. `lokal env` does that. It works the same way on
Linux, where browsers and tools trust lokal through the system certificate store instead: see
[On Linux](#on-linux).

## Set it up in your shell

```console
$ eval "$(lokal env)"                 # zsh, bash
$ lokal env --shell fish | source     # fish
```

Run it in the shell that starts your dev servers and scripts, **before** you start them. Node, for example, reads
`NODE_EXTRA_CA_CERTS` only at startup, so a process that was already running keeps failing until you restart it.

`lokal env` only prints shell exports, so you can see exactly what it sets:

```console
$ lokal env
export NODE_EXTRA_CA_CERTS='/Users/you/Library/Application Support/lokal/ca/root.pem'
export SSL_CERT_FILE='/Users/you/Library/Application Support/lokal/ca/bundle.pem'
export REQUESTS_CA_BUNDLE='/Users/you/Library/Application Support/lokal/ca/bundle.pem'
export CURL_CA_BUNDLE='/Users/you/Library/Application Support/lokal/ca/bundle.pem'
```

With `--shell fish`, the same variables come out as `set -gx` lines.

## What each variable does

| Variable | Read by | Points at |
|---|---|---|
| `NODE_EXTRA_CA_CERTS` | Node | lokal's CA certificate (`root.pem`). Node adds it to its built-in roots |
| `SSL_CERT_FILE` | OpenSSL, which Python's `ssl` module and `urllib` use | The bundle (`bundle.pem`) |
| `REQUESTS_CA_BUNDLE` | Python's `requests` library | The bundle |
| `CURL_CA_BUNDLE` | curl builds that use their own CA bundle | The bundle |

The last three variables replace a tool's whole list of trusted roots instead of adding to it. That's why they
point at a bundle that holds the macOS system roots plus lokal's CA: lokal URLs work, and public HTTPS sites keep
working too.

Setup writes the bundle. `lokal env` writes it again whenever it's more than 30 days old, so it picks up changes to
the macOS system roots.

## Make it permanent

Add the line to your shell profile, so every new shell gets it. For zsh, that's `~/.zshrc`. For bash, it's
`~/.bash_profile` or `~/.bashrc`:

```sh
eval "$(lokal env)"
```

For fish, add this to `~/.config/fish/config.fish`:

```fish
lokal env --shell fish | source
```

`lokal env` needs lokal to be set up. Before setup, it prints `lokal isn't set up yet` and sets nothing, so run
[`lokal setup`](/docs/setup) first. The same goes after `lokal uninstall`: the CA files stay on disk, but
`lokal env` stops exporting them, so a leftover line in your profile can't keep trusting lokal's CA. Uninstall
reminds you to remove the line.

## Use the CA file directly: `lokal ca path`

`lokal ca path` prints the path of lokal's CA certificate. Use it to set a variable for a single command:

```console
$ lokal ca path
/Users/you/Library/Application Support/lokal/ca/root.pem
$ NODE_EXTRA_CA_CERTS="$(lokal ca path)" node app.js
```

The path contains a space, so keep the quotes when you use it in a shell.

Recent Node versions can also use the macOS keychain directly, with `node --use-system-ca`. Then Node needs no
environment variable at all.

## Which tools need it

| Tool | Trusts lokal after setup? |
|---|---|
| Safari, Chrome and other browsers that use the keychain | Yes |
| Firefox | Yes. Firefox has its own trust store, so setup turns on its `security.enterprise_roots.enabled` preference in each profile |
| macOS tools, including `/usr/bin/curl` | Yes |
| Node | With `lokal env`, or `node --use-system-ca` |
| Python (`requests`, `urllib`) | With `lokal env` |
| Other curl builds | With `lokal env` (`CURL_CA_BUNDLE`), if they use their own bundle |

The Firefox preference makes Firefox trust every root CA in the macOS keychain, not only lokal's. The
[security model](/docs/concepts/security-model#firefox) explains the trade-off and how to undo it. If you create a
Firefox profile after setup, run `lokal setup` again to cover it.

## When it still fails

Errors like `unable to get local issuer certificate`, `self-signed certificate in certificate chain` or
`CERTIFICATE_VERIFY_FAILED` mean the tool doesn't trust lokal's CA yet.

- Make sure you ran `eval "$(lokal env)"` in the same shell, before the process started.
- Run `lokal doctor`. If `ca.trusted` fails, the CA isn't trusted by macOS either: run `lokal setup`, which
  re-trusts it and asks for your password.
- A handshake error rather than a certificate warning (curl says `tlsv1 alert access denied`) usually means the
  name isn't configured. Check `lokal list`.

[Troubleshooting](/docs/troubleshooting#node-python-or-another-tool-rejects-the-certificate) has more.

## On Linux

`lokal env` prints the same four variables. The paths are under `~/.local/share/lokal/ca/`, and the bundle holds
the system bundle (`/etc/ssl/certs/ca-certificates.crt` on Debian and Ubuntu, `/etc/pki/tls/certs/ca-bundle.crt`
on Fedora) plus lokal's CA:

```console
$ lokal env
export NODE_EXTRA_CA_CERTS='/home/you/.local/share/lokal/ca/root.pem'
export SSL_CERT_FILE='/home/you/.local/share/lokal/ca/bundle.pem'
export REQUESTS_CA_BUNDLE='/home/you/.local/share/lokal/ca/bundle.pem'
export CURL_CA_BUNDLE='/home/you/.local/share/lokal/ca/bundle.pem'
```

What trusts lokal after setup is different from macOS:

| Tool | Trusts lokal after setup? |
|---|---|
| curl, OpenSSL and other tools that read the system bundle | Yes. Setup adds lokal's CA to the system certificate store |
| Chrome, Chromium and Firefox | Yes, if `certutil` was installed when setup ran. Setup adds lokal's CA to each browser's own certificate database |
| Node | With `lokal env` |
| Python's `requests` | With `lokal env`, because it ships its own bundle |

Browsers don't read the system bundle, which is why setup needs `certutil` (`libnss3-tools` on Debian and Ubuntu,
`nss-tools` on Fedora). If it was missing, install it and run `lokal setup` again. There's no Firefox preference on
Linux: setup adds only lokal's CA to Firefox, not every root in the system store. If you create a Firefox profile
after setup, run `lokal setup` again to cover it.

[Troubleshooting](/docs/troubleshooting#on-linux) covers the Linux errors.
