#!/usr/bin/env bash
# Builds the release tarball for one macOS target:
#   dist/lokal-<version>-<target>.tar.gz and dist/lokal-<version>-<target>.tar.gz.sha256
# Prints the tarball's path on stdout; build output goes to stderr.
#
# Usage: scripts/package.sh <aarch64-apple-darwin|x86_64-apple-darwin>
set -euo pipefail

target="${1:?usage: scripts/package.sh <aarch64-apple-darwin|x86_64-apple-darwin>}"
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *)
    echo "error: unsupported target '$target'" >&2
    exit 2
    ;;
esac

cd "$(dirname "$0")/.."

# path+file:///…/crates/lokal#0.1.0 (or …#lokal@0.1.0) → 0.1.0
version="$(cargo pkgid -p lokal)"
version="${version##*[#@]}"

cargo build --release --locked -p lokal --target "$target" >&2
bin="target/$target/release/lokal"

# Ad-hoc signature (no Apple Developer ID yet). The linker already signs arm64
# binaries; this gives x86_64 the same and fails if the result doesn't verify.
codesign --force --sign - "$bin" 2>/dev/null
codesign --verify --strict "$bin"

actual="$(lipo -archs "$bin")"
if [ "$actual" != "$arch" ]; then
  echo "error: $bin is built for '$actual', expected '$arch'" >&2
  exit 1
fi

name="lokal-$version-$target"
mkdir -p dist
stage="dist/.stage-$target"
rm -rf "$stage"
mkdir -p "$stage/$name"
cp "$bin" README.md LICENSE CHANGELOG.md "$stage/$name/"
# No AppleDouble/xattr entries and no local user names in the archive.
tar --no-mac-metadata --no-xattrs --uid 0 --gid 0 --uname root --gname wheel \
  -C "$stage" -czf "dist/$name.tar.gz" "$name"
rm -rf "$stage"
(cd dist && shasum -a 256 "$name.tar.gz" >"$name.tar.gz.sha256")

echo "dist/$name.tar.gz"
