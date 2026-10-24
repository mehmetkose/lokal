#!/bin/bash
# Read-only check after `lokal uninstall` (docs/TESTING.md, "Clean room").
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p testkit --bin clean-check --quiet
exec ./target/release/clean-check "$@"
