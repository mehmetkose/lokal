#!/bin/bash
# Live test against the installed lokal (docs/TESTING.md). Builds the release
# binaries, then runs the Rust runner; arguments pass through (e.g. --full).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --workspace --quiet
exec ./target/release/live-test "$@"
