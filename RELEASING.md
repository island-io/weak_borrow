# Releasing

As a contributor you don't need to read this document. It documents operational
practices around the repo.

## Overview

Versions and the changelog are managed by hand. The only automation is
publishing, triggered by publishing a GitHub Release on a `vX.Y.Z` tag. The
`release.yml` workflow re-runs CI, then publishes to crates.io.

## Releasing a new version

- Branch from `main`.
- Edit `version` in `Cargo.toml`.
- In `CHANGELOG.md`, rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and
  add a fresh empty `## [Unreleased]` above it.
- Run `cargo check`.
- Open a PR titled `Release vX.Y.Z`. Merge it.
- On GitHub: Releases -> Draft a new release.
  - Choose a tag: create `vX.Y.Z`, target `main`.
  - Release title: `vX.Y.Z`.
  - Body: paste the `CHANGELOG.md` section for this version.
  - Tick "Set as a pre-release" if the version has a suffix like `-rc.1`.
  - Publish release.

## Patching an older version

Example: 0.2.0 is out and 0.1.x needs a fix.

- If the maintenance branch does not exist yet, create it from the last tag of
  that line.  GitHub: Branches -> New branch -> source `v0.1.1`. Only
  maintainers can create `release/**` branches, and all further work lands via
  PR.
- Open a PR titled `Release vX.Y.Z` into `release/0.1.x`. Merge it.
- Releases -> Draft a new release and follow instructions above.

Notes:

- `CHANGELOG.md` on `main` should also get the 0.1.2 entry so history stays
  complete.

## Yanking

Yanking is not automated.

1. Create a crates.io API token scoped to `yank`.
2. Run locally:

   ```sh
   cargo login
   cargo yank --version X.Y.Z
   ```

3. Revoke the token.

## Repo setup

Push access must not allow publishing a release, or landing code that gets
published without review.

### GitHub repository settings

Branch ruleset. Settings -> Rules -> Rulesets -> New ruleset -> New branch
ruleset:

- Name: `protected-branches`
- Target branches: Include default, include by pattern `release/**/*`
- Restrict deletions
- Require linear history
- Require a pull request before merging
- Require status checks to pass
- Block force pushes

Release branch ruleset. A second branch ruleset, so that its bypass list does
not also bypass the pull request rule above:

- Name: `release-branch-creation`
- Bypass list: Maintainers
- Target branches: Include by pattern `release/**/*`
- Restrict creations

Tag ruleset. Settings -> Rules -> Rulesets -> New ruleset -> New tag ruleset:

- Name: `tags`
- Bypass list: Maintainers
- Target tags: All
- Restrict creations
- Restrict updates
- Restrict deletions

A GitHub Release needs a tag, so this also stops non-maintainers from creating
Releases on new versions.

Environment. Settings -> Environments -> New environment:

- Name: `crates-io`
- Required reviewers: Maintainers
- Allow administrators to bypass configured protection rules: off
- Deployment branches and tags: Selected branches and tags
  - Add deployment branch or tag rule -> Ref type: Tag -> Name pattern: `v*`
  - No branch rules.

Actions. Settings -> Actions -> General:

- Workflow permissions: Read repository contents and packages permissions

### First publish and Trusted Publishing

The crate must exist on crates.io before a trusted publisher can be
configured, so the first publish (0.1.0) is manual.

- Prepare and merge `Release v0.1.0` as in "Releasing a new version".
- On crates.io create a new token.
- Locally, on the merged `main` commit:

   ```sh
   scripts/release-check.sh v0.1.0
   cargo login
   cargo publish --locked
   ```

- On crates.io: `weak_borrow` -> Settings -> Trusted Publishing -> Add
  GitHub:
  - Repository owner: `island-io`
  - Repository name: `weak_borrow`
  - Workflow filename: `release.yml`
  - Environment name: `crates-io`
- Revoke the API token.
- Tag the published commit so the tag history is complete. Do not create a
  GitHub Release for it, that would run the workflow against an already
  published version.
- Subsequent releases go through the workflow.
