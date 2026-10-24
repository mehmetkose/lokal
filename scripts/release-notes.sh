#!/usr/bin/env bash
# Prints the body of one version's section in CHANGELOG.md, for the GitHub
# release notes. Fails if the section is missing or empty.
#
# Usage: scripts/release-notes.sh <version>     (e.g. 0.1.0, without the v)
set -euo pipefail

version="${1:?usage: scripts/release-notes.sh <version>}"
changelog="$(dirname "$0")/../CHANGELOG.md"

# From "## [<version>]" (optionally followed by " - <date>") up to the next
# "## [" heading or the link references at the end of the file.
notes="$(awk -v h="## [$version]" '
  !found && ($0 == h || index($0, h " ") == 1) { found = 1; next }
  found && (/^## \[/ || /^\[[A-Za-z0-9.-]+\]: /) { exit }
  found { print }
' "$changelog" | sed '/./,$!d')"

if ! printf '%s' "$notes" | grep -q '[^[:space:]]'; then
  echo "error: CHANGELOG.md has no notes for $version (expected a '## [$version]' section)" >&2
  exit 1
fi

printf '%s\n' "$notes"
