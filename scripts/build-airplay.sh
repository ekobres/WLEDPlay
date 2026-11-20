#!/usr/bin/env bash
# Backward-compatible name; this builds Shairport Sync and NQPTP dependencies.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec "$ROOT/scripts/build-airplay-deps.sh" "$@"
