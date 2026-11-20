# Developer workflow notes

This repo now uses two workflow lanes:

- `CI` (`.github/workflows/ci.yml`) runs on:
  - pull requests
  - pushes to `main` and `release/**`
- `Publish container image` (`.github/workflows/docker-publish.yml`) runs only on version tags `v*`.

## Development branch and CI

Use `dev` for iterative work that should not run push-triggered CI. The CI workflow only runs for pushes to `main` and `release/**`, plus pull requests. The GHCR publish workflow is tag-triggered only. For multi-platform container testing, use `scripts/build-dev.sh` after committing and pushing a clean `dev` commit.

## Optional one-off CI skip on tracked push branches

`CI` has an explicit guard for push commits containing either marker:

- `[skip ci]`
- `[ci skip]`

Use this only for non-code commits on `main` or `release/**` when a push run should be skipped.

## Release-candidate and release flow

1. Prepare changes on a release branch (for example `release/0.3.1-rc.1`).
2. Bump `Cargo.toml` and `Cargo.lock` version.
3. Tag with `vX.Y.Z` or prerelease tag like `v0.3.1-rc.1`.
4. Push branch and tag.
5. Verify GHCR publish workflow and package visibility.
