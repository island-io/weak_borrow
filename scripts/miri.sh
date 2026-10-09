#!/bin/sh
# Locally: rustup +nightly component add miri
set -eu

MIRIFLAGS="${MIRIFLAGS:--Zmiri-many-seeds=0..32}" cargo +nightly miri test --locked
