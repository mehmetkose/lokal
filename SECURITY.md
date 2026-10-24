# Security policy

lokal installs a certificate authority that your Mac trusts, and runs a service that terminates TLS for you.
Security reports are taken seriously and handled ahead of other work.

## Reporting a vulnerability

**Please don't open a public issue.** Report it privately through GitHub instead:

1. Go to [Security → Report a vulnerability](https://github.com/mehmetkose/lokal/security/advisories/new).
2. Describe the problem, how to reproduce it, and what an attacker could do with it.
3. Include `lokal --version` and your macOS version (`sw_vers`).

The maintainer aims to acknowledge reports within a week. Once a fix is ready, it ships in a release, and the
advisory is published with credit to you (unless you prefer otherwise).

## Supported versions

lokal is pre-1.0. Only the latest release gets security fixes.

## What's in scope

Anything that breaks a guarantee in [`docs/SECURITY-MODEL.md`](docs/SECURITY-MODEL.md), for example:

- getting lokal's CA, or a leaf it issues, accepted for a name outside `.lokal` and `.local`, or for an IP
  address;
- making lokal issue a certificate for a name nobody configured;
- reading or exfiltrating the CA's private key through lokal (for example through logs, IPC, error pages or file
  permissions);
- reaching lokal's listeners (443, 80, 53535/udp) or its IPC socket from another machine or another user account;
- making `setup` or `uninstall` run a privileged command other than the ones they print, or with arguments an
  attacker controls (through `state.toml`, file names or environment variables, for example);
- making lokal's DNS server answer for names outside `.lokal`;
- leaving the system trusting a lokal CA after `lokal uninstall`.

## What's out of scope

- Attacks that need code already running as your user account. That code can read your files, including the
  CA key, directly. The name constraints are what limit the damage in that case, and a way around them is in scope.
- Reports about `.lokal` not being a reserved TLD. That's a known, documented trade-off. `lokal doctor` checks
  for it.
- Vulnerabilities in your own dev servers that lokal proxies to.
