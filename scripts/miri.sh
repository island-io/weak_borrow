#!/bin/sh
# Locally: rustup +nightly component add miri
set -eu

cargo +nightly miri test --locked
