# How lokal v0.1 was built

lokal v0.1 was written by a coding agent working from a specification, on the maintainer's Mac, with the
maintainer stepping in only for the steps that need a password. These files are the record of that build. They
aren't instructions for working on lokal today. For that, see
[`CONTRIBUTING.md`](../../CONTRIBUTING.md).

- [`../SPEC.md`](../SPEC.md): the specification the agent built from. It is still maintained, and still the
  source of truth.
- [`AGENTS-bootstrap.md`](AGENTS-bootstrap.md): the agent's original instructions. They set out the build loop,
  the boundaries on the developer's Mac (no sudo, no keychain changes), and two human gates. At **G1** the
  developer ran `lokal setup` to trust the CA. At **G2** they ran a final uninstall-then-setup "clean room" check.
- [`PROGRESS.md`](PROGRESS.md): the build log. It records the milestone status with evidence, the **VERIFY**
  experiments that proved macOS behaviour before anything was built on it, every **decision** that deviated from
  the spec (with measurements), and an iteration log.

Some findings from the log that shaped the design:

- launchd can hand ports 443 and 80 to an unprivileged LaunchAgent, but its default 10-second throttle delays
  restarting a crashed daemon. `ThrottleInterval` 1 brings that down to milliseconds.
- mDNSResponder accepts local-only records only under `.local`. A custom TLD needs its own DNS server and
  `/etc/resolver`.
- mDNSResponder caches *every* kind of failed lookup for about 75 seconds, which is why `.lokal` answers every
  name with loopback.
- `security verify-cert -r <root>` treats the root as an anchor and reports success even when the keychain doesn't
  trust it.
- DNS-only name constraints left IP addresses unconstrained, so the CA now excludes `0.0.0.0/0` and `::/0`.
