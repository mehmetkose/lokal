#!/usr/bin/env bash
# Fails unless the tag being released matches the lokal version in Cargo.toml.
# release.yml's build jobs run it on tag pushes.
#
# Usage: scripts/check-tag.sh [tag]     (default: $GITHUB_REF_NAME, e.g. v0.5.1)
set -euo pipefail

tag="${1:-${GITHUB_REF_NAME:?usage: scripts/check-tag.sh <tag>}}"
cd "$(dirname "$0")/.."

# path+file:///…/crates/lokal#0.1.0 (or …#lokal@0.1.0) → 0.1.0
version="$(cargo pkgid -p lokal)"
version="${version##*[#@]}"
if [ "$tag" != "v$version" ]; then
  echo "::error::tag $tag doesn't match the lokal version in Cargo.toml ($version)"
  exit 1
fi
