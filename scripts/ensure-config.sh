#!/usr/bin/env bash
# Create gitignored local config from examples when missing.
# Pulls update the examples; they never overwrite an existing local file.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

copy_if_missing() {
  src=$1
  dest=$2
  label=$3
  if [[ -f "$dest" ]]; then
    return 0
  fi
  if [[ ! -f "$src" ]]; then
    echo "Missing example $src"
    return 1
  fi
  cp "$src" "$dest"
  echo "Created $dest from $(basename "$src") — ${label}"
}

copy_if_missing \
  "$ROOT/config/config-example.yaml" \
  "$ROOT/config/config.yaml" \
  "edit destinations, NICs, and delay; gitignored."

copy_if_missing \
  "$ROOT/config/shairport-sync-example.conf" \
  "$ROOT/config/shairport-sync.conf" \
  "edit Shairport buffers/session; gitignored."
