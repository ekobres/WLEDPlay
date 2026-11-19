#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

fail() {
  echo "build-dev: $*" >&2
  exit 1
}

branch="$(git branch --show-current)"
[[ "$branch" == "dev" ]] || fail "switch to the dev branch before publishing a dev image"
[[ -z "$(git status --porcelain --untracked-files=all)" ]] || fail "worktree must be clean; commit and push source changes first"

git fetch --quiet origin refs/heads/dev:refs/remotes/origin/dev
commit="$(git rev-parse HEAD)"
remote_commit="$(git rev-parse refs/remotes/origin/dev)"
[[ "$commit" == "$remote_commit" ]] || fail "local dev is not at origin/dev; push or pull before building"

builder="${WLEDPLAY_BUILDX_BUILDER:-wledplay-dev}"
if ! docker buildx inspect "$builder" >/dev/null 2>&1; then
  docker buildx create --name "$builder" --driver docker-container >/dev/null
fi
docker buildx inspect "$builder" --bootstrap

if [[ -n "${WLEDPLAY_DEV_IMAGE:-}" ]]; then
  image="$WLEDPLAY_DEV_IMAGE"
else
  origin_url="$(git remote get-url origin)"
  case "$origin_url" in
    https://*|http://*|ssh://*)
      repo_path="${origin_url#*://}"
      repo_path="${repo_path#*/}"
      ;;
    *@*:*) repo_path="${origin_url#*:}" ;;
    *) fail "cannot derive GHCR owner from origin URL; set WLEDPLAY_DEV_IMAGE=ghcr.io/OWNER/wledplay" ;;
  esac
  repo_path="${repo_path%.git}"
  owner="${repo_path%%/*}"
  [[ "$owner" != "$repo_path" && -n "$owner" ]] || fail "cannot derive repository owner from origin URL; set WLEDPLAY_DEV_IMAGE=ghcr.io/OWNER/wledplay"
  owner="$(printf '%s' "$owner" | tr '[:upper:]' '[:lower:]')"
  image="ghcr.io/${owner}/wledplay"
fi
sha="${commit:0:12}"
platforms="${WLEDPLAY_DEV_PLATFORMS:-linux/amd64,linux/arm64}"
shairport_patch="${WLEDPLAY_SHAIRPORT_PATCH:-0}"
nqptp_patch="${WLEDPLAY_NQPTP_PATCH:-0}"

echo "Building ${platforms} from dev commit ${sha}"
echo "Patch args: Shairport=${shairport_patch}, NQPTP=${nqptp_patch}"
docker buildx build \
  --builder "$builder" \
  --platform "$platforms" \
  --build-arg "WLEDPLAY_SHAIRPORT_PATCH=${shairport_patch}" \
  --build-arg "WLEDPLAY_NQPTP_PATCH=${nqptp_patch}" \
  --tag "${image}:dev" \
  --tag "${image}:dev-${sha}" \
  --provenance=true \
  --sbom=true \
  --push \
  "$ROOT"

docker buildx imagetools inspect "${image}:dev-${sha}" >/dev/null
echo "Published ${image}:dev and ${image}:dev-${sha}"
echo "On test clients, set WLEDPLAY_IMAGE=${image}:dev-${sha}, then run docker compose pull and docker compose up -d --no-build."