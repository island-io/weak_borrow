#!/bin/sh
# Usage: scripts/release-check.sh vX.Y.Z
set -eu
cd "$(dirname "$0")/.."

version=${1?usage: $0 vX.Y.Z}
version=${version#v}
manifest=$(cargo pkgid | sed 's/.*[#@]//')

[ "$version" = "$manifest" ] ||
    { echo "release-check: tag is $version but Cargo.toml says $manifest" >&2; exit 1; }
grep -q "^## \[$version\] - [0-9]" CHANGELOG.md ||
    { echo "release-check: CHANGELOG.md has no '## [$version] - <date>' section" >&2; exit 1; }
cargo publish --dry-run --locked
echo "release-check: $version OK"
