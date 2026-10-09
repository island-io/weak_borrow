#!/bin/sh
# Usage: scripts/ci.sh [--no-lint]
set -eu

if [ "${1:-}" != --no-lint ]; then
    cargo fmt --check
    cargo clippy --all-targets --locked -- -D warnings
fi
cargo test --locked
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --locked
