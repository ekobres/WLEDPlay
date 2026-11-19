# Developer workflow notes

This repo now uses two workflow lanes:

- `CI` (`.github/workflows/ci.yml`) runs on:
  - pull requests
  - pushes to `main` and `release/**`
- `Publish container image` (`.github/workflows/docker-publish.yml`) runs only on version tags `v*`.

## Sync files to a remote dev box without noisy CI

If you push mainly to transfer work to another machine, avoid pushing those commits to `main`.

Recommended pattern:

1. Create a sync branch (once): `sync/<your-name>` or `dev/<topic>`.
2. Push WIP commits to that branch.
3. Pull the same branch on the remote dev box.

Because CI push triggers are limited to `main` and `release/**`, these sync branches will not run CI on every push.

## Optional one-off CI skip on tracked push branches

`CI` has an explicit guard for push commits containing either marker:

- `[skip ci]`
- `[ci skip]`

Use this only for non-code or transfer-only commits on `main`/`release/**`.

## Release-candidate and release flow

1. Prepare changes on a release branch (for example `release/0.3.1-rc.1`).
2. Bump `Cargo.toml` and `Cargo.lock` version.
3. Tag with `vX.Y.Z` or prerelease tag like `v0.3.1-rc.1`.
4. Push branch and tag.
5. Verify GHCR publish workflow and package visibility.
