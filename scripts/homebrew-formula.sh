#!/usr/bin/env bash
# Renders the Homebrew formula for a release from packaging/homebrew/lokal.rb.in,
# taking the four tarballs' checksums (macOS and Linux, arm64 and x86_64) from the
# release's SHA256SUMS file.
#
# Usage: scripts/homebrew-formula.sh <version> <SHA256SUMS> [URL base]   (version without the v)
#   The URL base defaults to the release's download URL. release.yml passes a
#   file:// directory to install the formula from a run's own tarballs.
set -euo pipefail

usage="usage: scripts/homebrew-formula.sh <version> <SHA256SUMS> [URL base]"
version="${1:?$usage}"
sums="${2:?$usage}"
url_base="${3:-https://github.com/mehmetkose/lokal/releases/download/v$version}"
template="$(dirname "$0")/../packaging/homebrew/lokal.rb.in"

if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: '$version' is not a release version (X.Y.Z)" >&2
  exit 2
fi

sha_for() {
  local asset="lokal-$version-$1.tar.gz" sha
  sha="$(awk -v a="$asset" '$2 == a || $2 == "*" a { print $1 }' "$sums")"
  if ! [[ "$sha" =~ ^[0-9a-f]{64}$ ]]; then
    echo "error: no sha256 for $asset in $sums" >&2
    return 1
  fi
  printf '%s\n' "$sha"
}

macos_arm="$(sha_for aarch64-apple-darwin)"
macos_intel="$(sha_for x86_64-apple-darwin)"
linux_arm="$(sha_for aarch64-unknown-linux-gnu)"
linux_intel="$(sha_for x86_64-unknown-linux-gnu)"

formula="$(sed -e "s|@URL_BASE@|$url_base|g" \
  -e "s/@VERSION@/$version/g" \
  -e "s/@SHA256_MACOS_ARM@/$macos_arm/g" \
  -e "s/@SHA256_MACOS_INTEL@/$macos_intel/g" \
  -e "s/@SHA256_LINUX_ARM@/$linux_arm/g" \
  -e "s/@SHA256_LINUX_INTEL@/$linux_intel/g" "$template")"

if printf '%s' "$formula" | grep -q '@[A-Z0-9_]*@'; then
  echo "error: $template has a placeholder this script doesn't fill" >&2
  exit 1
fi

printf '%s\n' "$formula"
