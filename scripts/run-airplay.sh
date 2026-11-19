#!/usr/bin/env bash
# Backward-compatible name for the WLEDPlay native runner.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec "$ROOT/scripts/run-wledplay.sh" "$@"
