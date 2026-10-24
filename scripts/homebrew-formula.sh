#!/usr/bin/env bash
# Renders the Homebrew formula for a release from packaging/homebrew/lokal.rb.in,
# taking both tarballs' checksums from the release's SHA256SUMS file.
#
# Usage: scripts/homebrew-formula.sh <version> <SHA256SUMS>   (version without the v)
set -euo pipefail

version="${1:?usage: scripts/homebrew-formula.sh <version> <SHA256SUMS>}"
sums="${2:?usage: scripts/homebrew-formula.sh <version> <SHA256SUMS>}"
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

arm="$(sha_for aarch64-apple-darwin)"
intel="$(sha_for x86_64-apple-darwin)"

formula="$(sed -e "s/@VERSION@/$version/g" \
  -e "s/@SHA256_ARM@/$arm/g" \
  -e "s/@SHA256_INTEL@/$intel/g" "$template")"

if printf '%s' "$formula" | grep -q '@[A-Z0-9_]*@'; then
  echo "error: $template has a placeholder this script doesn't fill" >&2
  exit 1
fi

printf '%s\n' "$formula"
