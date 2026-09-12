#!/usr/bin/env bash
# Phase 4: NQPTP + Shairport Sync (AirPlay 2) → WLEDPlay → WLED
# Requires Linux (or FreeBSD/OpenBSD). macOS cannot host official AP2.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ "$(uname -s)" == "Darwin" ]]; then
  cat <<'EOF'
Official AirPlay 2 cannot run on this Mac.

Shairport Sync AP2 uses NQPTP, which needs exclusive UDP ports 319 and 320.
macOS already uses those ports for its own PTP stack
(https://github.com/mikebrady/shairport-sync/blob/master/AIRPLAY2.md).

Keep using AirPlay 1 here:
  ./scripts/run-airplay.sh

For AirPlay 2 multi-room, run this script on a Linux host (Pi, NUCs, etc.):
  ./scripts/build-airplay2.sh
  ./scripts/run-airplay2.sh

Or on that Linux host:
  WLEDPLAY_DESTINATION=10.11.1.124 docker compose up -d --build
EOF
  exit 1
fi

SHAIRPORT_BIN="${SHAIRPORT_BIN:-$ROOT/vendor/bin/shairport-sync}"
NQPTP_BIN="${NQPTP_BIN:-$ROOT/vendor/bin/nqptp}"
if [[ ! -x "$SHAIRPORT_BIN" ]] && command -v shairport-sync >/dev/null 2>&1; then
  SHAIRPORT_BIN="$(command -v shairport-sync)"
fi
if [[ ! -x "$NQPTP_BIN" ]] && command -v nqptp >/dev/null 2>&1; then
  NQPTP_BIN="$(command -v nqptp)"
fi

WLEDPLAY_BIN="${WLEDPLAY_BIN:-$ROOT/target/release/wledplay}"
CONFIG="${WLEDPLAY_CONFIG:-$ROOT/config/config.yaml}"
SHAIRPORT_CONF="${SHAIRPORT_CONF:-$ROOT/config/shairport-sync-airplay2.conf}"
AIRPLAY_NAME="${WLEDPLAY_AIRPLAY_NAME:-WLEDPlay}"

if [[ ! -x "$WLEDPLAY_BIN" ]]; then
  echo "Building wledplay..."
  env -u CARGO_TARGET_DIR cargo build --release
  WLEDPLAY_BIN="$ROOT/target/release/wledplay"
fi

if [[ ! -x "$SHAIRPORT_BIN" ]] || ! "$SHAIRPORT_BIN" -V 2>/dev/null | grep -Eqi 'AirPlay2|airplay-2'; then
  echo "Need an AirPlay 2 shairport-sync (version string contains AirPlay2)."
  echo "Build it with: ./scripts/build-airplay2.sh"
  echo "Current binary: ${SHAIRPORT_BIN}"
  "$SHAIRPORT_BIN" -V 2>/dev/null || true
  exit 1
fi

if [[ ! -x "$NQPTP_BIN" ]]; then
  echo "nqptp not found. Build it with: ./scripts/build-airplay2.sh"
  exit 1
fi

if ! "$SHAIRPORT_BIN" -V 2>/dev/null | grep -q stdout; then
  echo "This shairport-sync build has no stdout backend."
  exit 1
fi

TMP_CONF="$(mktemp -t wledplay-shairport.XXXXXX.conf)"
NQPTP_PID=""
cleanup() {
  rm -f "$TMP_CONF"
  if [[ -n "${NQPTP_PID}" ]]; then
    kill "$NQPTP_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

sed "s/name = \"WLEDPlay\"/name = \"${AIRPLAY_NAME}\"/" "$SHAIRPORT_CONF" > "$TMP_CONF"

echo "Starting nqptp (${NQPTP_BIN})..."
"$NQPTP_BIN" &
NQPTP_PID=$!
sleep 0.4
if ! kill -0 "$NQPTP_PID" 2>/dev/null; then
  echo "nqptp exited immediately. It needs UDP 319/320 (often root or cap_net_bind_service)."
  exit 1
fi

echo "Advertising AirPlay 2 receiver: ${AIRPLAY_NAME}"
echo "PCM: 48000 Hz S16LE stereo (pinned). Group this destination with HomePods to measure sync."

"$SHAIRPORT_BIN" -c "$TMP_CONF" --service-type=airplay2 -o stdout | \
  "$WLEDPLAY_BIN" --config "$CONFIG" --input stdin --no-realtime \
    --pcm-rate 48000 --pcm-channels 2 --pcm-format s16le
