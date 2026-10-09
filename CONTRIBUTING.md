# Contributing

## Local checks

Run the same checks as CI before opening a PR:

```sh
scripts/ci.sh
scripts/miri.sh
```

Miri needs nightly:

```sh
rustup toolchain install nightly
rustup +nightly component add miri
```

## Pull requests

- User-visible changes get a line under `## [Unreleased]` in `CHANGELOG.md`.
- Every `unsafe` block has a `// SAFETY:` comment.
