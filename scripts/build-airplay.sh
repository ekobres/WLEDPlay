#!/usr/bin/env bash
# Build NQPTP + Shairport Sync (AirPlay 2, stdout) into vendor/bin.
# Linux / FreeBSD only. See AIRPLAY2.md: not supported on macOS.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
"$ROOT/scripts/ensure-config.sh"
PREFIX="${WLEDPLAY_PREFIX:-$ROOT/vendor}"
# Upstream base: mikebrady/shairport-sync 5.6.3 (7ce90d89).
# The fork's master tracks that upstream. This script builds the seamless-group-join
# branch. SHAIRPORT_REF may be a branch name or a commit.
SHAIRPORT_REPO="${SHAIRPORT_REPO:-https://github.com/ekobres/shairport-sync.git}"
SHAIRPORT_REF="${SHAIRPORT_REF:-seamless-group-join}"
NQPTP_VERSION="${NQPTP_VERSION:-1.2.9}"
SRC="$PREFIX/src"
mkdir -p "$PREFIX/bin" "$SRC"

if [[ "$(uname -s)" == "Darwin" ]]; then
  echo "Skipping compile: official AirPlay 2 + NQPTP cannot run on macOS (UDP 319/320)."
  echo "Run this script on the Linux host that will advertise WLEDPlay."
  exit 1
fi

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing build tool: $1"
    echo "On a Docker-only host, skip this script and use:"
    echo "  WLEDPLAY_DESTINATION=<wled-ip> docker compose up -d --build"
    exit 1
  }
}
need git
need autoreconf
need make

clone_tag() {
  local url="$1" dest="$2" tag="$3"
  if [[ -d "$dest/.git" ]]; then
    git -C "$dest" fetch --depth 1 origin "refs/tags/${tag}:refs/tags/${tag}" || git -C "$dest" fetch --tags
    git -C "$dest" checkout -f "$tag"
  else
    git clone --depth 1 --branch "$tag" "$url" "$dest"
  fi
}

clone_ref() {
  local url="$1" dest="$2" ref="$3"
  if [[ ! -d "$dest/.git" ]]; then
    git init -q "$dest"
  fi
  if git -C "$dest" remote get-url origin >/dev/null 2>&1; then
    git -C "$dest" remote set-url origin "$url"
  else
    git -C "$dest" remote add origin "$url"
  fi
  git -C "$dest" fetch --depth 1 origin "$ref"
  git -C "$dest" checkout -f --detach FETCH_HEAD
}

echo "Building nqptp ${NQPTP_VERSION}..."
clone_tag https://github.com/mikebrady/nqptp.git "$SRC/nqptp" "$NQPTP_VERSION"
(
  cd "$SRC/nqptp"
  autoreconf -fi
  ./configure --prefix="$PREFIX"
  make -j"$(nproc 2>/dev/null || echo 4)"
  make install
)

echo "Building shairport-sync ${SHAIRPORT_REF} from ${SHAIRPORT_REPO} (AirPlay 2 + stdout)..."
clone_ref "$SHAIRPORT_REPO" "$SRC/shairport-sync" "$SHAIRPORT_REF"
(
  cd "$SRC/shairport-sync"
  autoreconf -fi
  ./configure --prefix="$PREFIX" \
    --with-stdout --with-airplay-2 --with-avahi --with-ssl=openssl --with-soxr --with-metadata
  make -j"$(nproc 2>/dev/null || echo 4)"
  make install
)

echo
echo "Installed:"
"$PREFIX/bin/nqptp" -V || true
"$PREFIX/bin/shairport-sync" -V || true
echo
echo "Next: ./scripts/run-airplay.sh"
