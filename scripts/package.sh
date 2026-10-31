#!/usr/bin/env bash
# Builds the release artifacts for one target into dist/:
#   lokal-<version>-<target>.tar.gz, on macOS and Linux;
#   on Linux also lokal_<version>_<arch>.deb and lokal-<version>-1.<arch>.rpm;
# each with a .sha256 file next to it.
# Prints the tarball's path on stdout; build output goes to stderr.
#
# Usage: scripts/package.sh <target>
#   macOS: aarch64-apple-darwin, x86_64-apple-darwin
#   Linux: x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu (built natively,
#          on Linux of the same architecture; needs dpkg-deb and rpmbuild)
set -euo pipefail

usage="usage: scripts/package.sh <aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu>"
target="${1:?$usage}"
case "$target" in
  aarch64-apple-darwin) os=macos arch=arm64 ;;
  x86_64-apple-darwin) os=macos arch=x86_64 ;;
  x86_64-unknown-linux-gnu)
    os=linux arch=x86_64 deb_arch=amd64
    machine='Advanced Micro Devices X86-64' loader=/lib64/ld-linux-x86-64.so.2
    ;;
  aarch64-unknown-linux-gnu)
    os=linux arch=aarch64 deb_arch=arm64
    machine=AArch64 loader=/lib/ld-linux-aarch64.so.1
    ;;
  *)
    echo "error: unsupported target '$target'" >&2
    echo "$usage" >&2
    exit 2
    ;;
esac

# The newest glibc symbol version a Linux binary may need. The build host's
# glibc sets it: the release builds on the ubuntu-24.04 runners (glibc 2.39).
# Every distribution that can run lokal setup (systemd 258+) has 2.42 or later,
# so there's no need to build on anything older. See docs/PACKAGING.md.
GLIBC_FLOOR=2.39

cd "$(dirname "$0")/.."

if [ "$os" = linux ]; then
  # No cross linker for ring's C and assembly: build each architecture on its own.
  host="$(uname -s)/$(uname -m)"
  if [ "$host" != "Linux/$arch" ]; then
    echo "error: build $target on $arch Linux (this is $host)" >&2
    exit 2
  fi
  for tool in readelf objdump dpkg-deb rpmbuild sha256sum; do
    if ! command -v "$tool" >/dev/null; then
      echo "error: $tool is missing (on Debian and Ubuntu: apt-get install binutils dpkg rpm coreutils)" >&2
      exit 2
    fi
  done
fi

# path+file:///…/crates/lokal#0.1.0 (or …#lokal@0.1.0) → 0.1.0
version="$(cargo pkgid -p lokal)"
version="${version##*[#@]}"

cargo build --release --locked -p lokal --target "$target" >&2
bin="target/$target/release/lokal"

name="lokal-$version-$target"
mkdir -p dist
stage="dist/.stage-$target"
rm -rf "$stage"
mkdir -p "$stage/$name"

if [ "$os" = macos ]; then
  # Ad-hoc signature (no Apple Developer ID yet). The linker already signs arm64
  # binaries; this gives x86_64 the same and fails if the result doesn't verify.
  codesign --force --sign - "$bin" 2>/dev/null
  codesign --verify --strict "$bin"

  actual="$(lipo -archs "$bin")"
  if [ "$actual" != "$arch" ]; then
    echo "error: $bin is built for '$actual', expected '$arch'" >&2
    exit 1
  fi

  cp "$bin" README.md LICENSE CHANGELOG.md "$stage/$name/"
  # No AppleDouble/xattr entries and no local user names in the archive.
  tar --no-mac-metadata --no-xattrs --uid 0 --gid 0 --uname root --gname wheel \
    -C "$stage" -czf "dist/$name.tar.gz" "$name"
  rm -rf "$stage"
  (cd dist && shasum -a 256 "$name.tar.gz" >"$name.tar.gz.sha256")

  echo "dist/$name.tar.gz"
  exit 0
fi

# --- Linux ---

# The binary: architecture, loader, shared libraries and glibc floor.
if ! readelf -h "$bin" | grep -q "Machine: *$machine\$"; then
  echo "error: $bin isn't built for $machine:" >&2
  readelf -h "$bin" | grep Machine >&2
  exit 1
fi
if ! readelf -l "$bin" | grep -qF "[Requesting program interpreter: $loader]"; then
  echo "error: $bin doesn't use the loader $loader:" >&2
  readelf -l "$bin" | grep -F 'program interpreter' >&2
  exit 1
fi
needed="$(readelf -d "$bin" | sed -n 's/.*(NEEDED).*\[\(.*\)\]$/\1/p')"
for lib in $needed; do
  case "$lib" in
    libc.so.6 | libm.so.6 | libgcc_s.so.1 | "${loader##*/}") ;;
    *)
      # A new library is a new runtime dependency: add it to the package
      # dependencies (packaging/linux/) and docs/PACKAGING.md, then here.
      echo "error: $bin links $lib, which the packages don't depend on" >&2
      exit 1
      ;;
  esac
done
glibc="$(objdump -T "$bin" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -V | tail -n 1)"
if [ -z "$glibc" ]; then
  echo "error: found no GLIBC symbol versions in $bin" >&2
  exit 1
fi
if [ "$(printf '%s\n%s\n' "$glibc" "$GLIBC_FLOOR" | sort -V | tail -n 1)" != "$GLIBC_FLOOR" ]; then
  echo "error: $bin needs glibc $glibc, newer than the floor, $GLIBC_FLOOR" >&2
  exit 1
fi
echo "$bin: $machine, $loader, needs glibc $glibc (floor $GLIBC_FLOOR), links: $(echo "$needed" | tr '\n' ' ')" >&2

# The tarball: the same layout as on macOS.
cp "$bin" README.md LICENSE CHANGELOG.md "$stage/$name/"
tar --owner=0 --group=0 --numeric-owner --sort=name -C "$stage" -czf "dist/$name.tar.gz" "$name"

# Package versions: a pre-release X.Y.Z-rc.1 becomes X.Y.Z~rc.1, which both dpkg
# and rpm sort before X.Y.Z.
pkgver="$(printf '%s' "$version" | tr - '~')"
summary="Real .lokal domains with valid HTTPS for local dev servers"

# The .deb: the binary and its docs, no maintainer scripts (docs/PACKAGING.md).
deb_libs="libc6 (>= $glibc)"
case " $(echo "$needed" | tr '\n' ' ') " in *" libgcc_s.so.1 "*) deb_libs="$deb_libs, libgcc-s1" ;; esac
root="$stage/deb"
install -D -m 0755 "$bin" "$root/usr/bin/lokal"
install -D -m 0644 README.md "$root/usr/share/doc/lokal/README.md"
install -D -m 0644 CHANGELOG.md "$root/usr/share/doc/lokal/CHANGELOG.md"
install -D -m 0644 packaging/linux/copyright "$root/usr/share/doc/lokal/copyright"
size="$(du -sk "$root" | cut -f1)"
mkdir -p "$root/DEBIAN"
sed -e "s|@VERSION@|$pkgver|g" -e "s|@ARCH@|$deb_arch|g" -e "s|@SIZE@|$size|g" \
  -e "s|@LIBS@|$deb_libs|g" -e "s|@SUMMARY@|$summary|g" packaging/linux/control.in >"$root/DEBIAN/control"
deb="lokal_${pkgver}_$deb_arch.deb"
dpkg-deb --root-owner-group -Zxz --build "$root" "dist/$deb" >&2

# The .rpm, from the same files. rpmbuild adds the glibc and libgcc requirements
# itself, from the binary.
top="$PWD/$stage/rpm"
mkdir -p "$top/SOURCES" "$top/SPECS"
cp "$bin" README.md LICENSE CHANGELOG.md "$top/SOURCES/"
sed -e "s|@VERSION@|$pkgver|g" -e "s|@SUMMARY@|$summary|g" packaging/linux/lokal.spec.in >"$top/SPECS/lokal.spec"
rpmbuild -bb --quiet --target "$arch" --define "_topdir $top" "$top/SPECS/lokal.spec" >&2
rpm="lokal-$pkgver-1.$arch.rpm"
cp "$top/RPMS/$arch/$rpm" "dist/$rpm"

rm -rf "$stage"
(cd dist && for f in "$name.tar.gz" "$deb" "$rpm"; do sha256sum "$f" >"$f.sha256"; done)

echo "dist/$name.tar.gz"
