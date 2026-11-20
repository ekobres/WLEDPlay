#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

"$ROOT/scripts/build-airplay-deps.sh"
env -u CARGO_TARGET_DIR cargo build --release