#!/usr/bin/env bash
# Smoke-tests one architecture's Linux release artifacts in dist/ (from
# scripts/package.sh) in clean containers of the distributions lokal targets:
#   - every checksum matches;
#   - the tarball's binary prints the version, here and on each distribution;
#   - the .deb installs on Ubuntu 26.04, and apt refuses it where systemd is
#     older than 258, which setup needs (Ubuntu 24.04, Debian 13);
#   - the .rpm installs on Fedora 43 and 44.
# Runs every image for the host's architecture, so run it on the target's.
#
# Usage: scripts/smoke-linux.sh <x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu> [git short hash]
#   With a hash, `lokal --version` must report it (a release build must not say nogit00).
#   DOCKER=podman to use podman instead of docker.
set -euo pipefail

usage="usage: scripts/smoke-linux.sh <x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu> [git short hash]"
target="${1:?$usage}"
hash="${2:-}"
case "$target" in
  x86_64-unknown-linux-gnu) arch=x86_64 deb_arch=amd64 ;;
  aarch64-unknown-linux-gnu) arch=aarch64 deb_arch=arm64 ;;
  *)
    echo "$usage" >&2
    exit 2
    ;;
esac
docker="${DOCKER:-docker}"

cd "$(dirname "$0")/.."

version="$(cargo pkgid -p lokal)"
version="${version##*[#@]}"
pkgver="$(printf '%s' "$version" | tr - '~')"
name="lokal-$version-$target"
deb="lokal_${pkgver}_$deb_arch.deb"
rpm="lokal-$pkgver-1.$arch.rpm"
want="lokal $version (${hash:+$hash }"
dist="$PWD/dist"

failed=0
pass() { echo "ok: $*"; }
fail() {
  echo "::error::$*"
  failed=1
}

# In a container: `lokal --version` must start with $WANT.
# shellcheck disable=SC2016 # expanded in the container, not here
check_version='out="$(lokal --version)"; case "$out" in "$WANT"*) echo "$out" ;; *) echo "expected \"$WANT…\", got \"$out\"" >&2; exit 1 ;; esac'

run() { # <image> <script>: runs the script in a fresh container with dist/ at /dist.
  "$docker" run --rm -v "$dist:/dist:ro" -e WANT="$want" -e DEBIAN_FRONTEND=noninteractive "$1" sh -euc "$2"
}

echo "checksums:"
for f in "$name.tar.gz" "$deb" "$rpm"; do
  if (cd "$dist" && sha256sum -c "$f.sha256"); then pass "$f"; else fail "$f doesn't match $f.sha256"; fi
done

echo "the tarball on this machine:"
if [ "$(uname -s)" = Linux ]; then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  tar -xzf "$dist/$name.tar.gz" -C "$tmp"
  if out="$("$tmp/$name/lokal" --version)" && [[ "$out" == "$want"* ]]; then
    pass "$out"
  else
    fail "expected '$want…' from the tarball's lokal --version, got '${out:-nothing}'"
  fi
else
  # A developer's Mac running the containers through podman: the images below still run it.
  echo "not Linux: only in the containers"
fi

# check <description> <image> <script>: the script must succeed in a fresh container. Shows the
# script's last line (the version) when it passes, and the end of its output when it doesn't.
check() {
  local out
  if out="$(run "$2" "$3" 2>&1)"; then
    pass "$1: $(tail -n 1 <<<"$out")"
  else
    fail "$1"
    tail -n 20 <<<"$out"
  fi
}

images=(docker.io/library/ubuntu:24.04 docker.io/library/debian:13 docker.io/library/ubuntu:26.04
  docker.io/library/fedora:44)
# Arch Linux publishes no arm64 image.
if [ "$arch" = x86_64 ]; then images+=(docker.io/library/archlinux:latest); fi
for image in "${images[@]}"; do
  check "the tarball runs on $image" "$image" \
    "tar -xzf /dist/$name.tar.gz -C /tmp && PATH=/tmp/$name:\$PATH && $check_version"
done

apt_install="apt-get update -qq && apt-get install -y --no-install-recommends /dist/$deb"

check "the .deb installs on Ubuntu 26.04" docker.io/library/ubuntu:26.04 \
  "$apt_install && dpkg -L lokal | grep -qx /usr/bin/lokal && $check_version"

for image in docker.io/library/ubuntu:24.04 docker.io/library/debian:13; do
  if out="$(run "$image" "$apt_install" 2>&1)"; then
    fail "$deb installed on $image, whose systemd is older than 258"
  elif grep -qF 'systemd (>= 258)' <<<"$out"; then
    pass "apt refuses the .deb on $image: $(grep -F 'systemd (>= 258)' <<<"$out" | head -n 1 | sed 's/^ *//')"
  else
    fail "apt refused $deb on $image, but not because of systemd"
    tail -n 20 <<<"$out"
  fi
done

for image in docker.io/library/fedora:43 docker.io/library/fedora:44; do
  check "the .rpm installs on $image" "$image" \
    "dnf install -y /dist/$rpm && rpm -ql lokal | grep -qx /usr/bin/lokal && $check_version"
done

exit "$failed"
