#!/usr/bin/env bash
# NQPTP + Shairport Sync (AirPlay 2) → WLEDPlay → WLED
# Requires Linux (or FreeBSD/OpenBSD). macOS cannot host official AP2.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
"$ROOT/scripts/ensure-config.sh"

if [[ "$(uname -s)" == "Darwin" ]]; then
  cat <<'EOF'
WLEDPlay requires AirPlay 2 with NQPTP for synchronized multi-room grouping.

Official AirPlay 2 cannot run on macOS (UDP ports 319/320 belong to the OS).
AirPlay 1 is deliberately not supported — it cannot join a live speaker group
with correct presentation-time sync.

Run WLEDPlay on a Linux host instead:
  ./scripts/build-wledplay.sh
  ./scripts/run-wledplay.sh

Or:
  WLEDPLAY_DESTINATION=192.0.2.10 docker compose up -d --build
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
if [[ ! -f "$CONFIG" ]]; then
  echo "Missing $CONFIG (copy config/config-example.yaml)."
  exit 1
fi
SHAIRPORT_CONF="${SHAIRPORT_CONF:-$ROOT/config/shairport-sync.conf}"
if [[ ! -f "$SHAIRPORT_CONF" ]]; then
  echo "Missing $SHAIRPORT_CONF (copy config/shairport-sync-example.conf)."
  exit 1
fi
AIRPLAY_NAME="${WLEDPLAY_AIRPLAY_NAME:-WLEDPlay}"

if [[ ! -x "$WLEDPLAY_BIN" ]]; then
  echo "Building wledplay..."
  env -u CARGO_TARGET_DIR cargo build --release
  WLEDPLAY_BIN="$ROOT/target/release/wledplay"
fi

if [[ ! -x "$SHAIRPORT_BIN" ]] || ! "$SHAIRPORT_BIN" -V 2>/dev/null | grep -Eqi 'AirPlay2|airplay-2'; then
  echo "Need an AirPlay 2 shairport-sync (version string contains AirPlay2)."
  echo "Build it with: ./scripts/build-wledplay.sh"
  echo "Current binary: ${SHAIRPORT_BIN}"
  "$SHAIRPORT_BIN" -V 2>/dev/null || true
  exit 1
fi

if [[ ! -x "$NQPTP_BIN" ]]; then
  echo "nqptp not found. Build it with: ./scripts/build-wledplay.sh"
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
OFFSET="${WLEDPLAY_SHAIRPORT_LATENCY_OFFSET:-}"
if [[ -z "$OFFSET" ]]; then
  OFFSET="$(sed -n 's/^[[:space:]]*shairport_latency_offset_s:[[:space:]]*//p' "$CONFIG" 2>/dev/null | head -1 | tr -d '[:space:]')"
fi
META_PIPE="${WLEDPLAY_METADATA_PIPE:-/tmp/wledplay.meta}"
python3 - "$TMP_CONF" "${WLEDPLAY_AIRPLAY_INTERFACE:-}" "$OFFSET" "$META_PIPE" <<'PY'
import pathlib, re, sys
p = pathlib.Path(sys.argv[1])
iface, offset, pipe = sys.argv[2], sys.argv[3], sys.argv[4]
text = p.read_text()
if iface:
    if re.search(r"(?m)^[ \t]*//?[ \t]*interface\s*=", text) or re.search(r"(?m)^[ \t]*interface\s*=", text):
        text = re.sub(r"(?m)^[ \t]*//[ \t]*interface\s*=.*$", f'  interface = "{iface}";', text)
        text = re.sub(r"(?m)^[ \t]*interface\s*=.*$", f'  interface = "{iface}";', text)
    else:
        text = text.replace("general = {", f'general = {{\n  interface = "{iface}";', 1)
if offset:
    text = re.sub(
        r"(?m)^[ \t]*audio_backend_latency_offset_in_seconds\s*=.*$",
        f"  audio_backend_latency_offset_in_seconds = {offset};",
        text,
    )
text = re.sub(r'(?m)^[ \t]*pipe_name\s*=.*$', f'  pipe_name = "{pipe}";', text)
p.write_text(text)
PY

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

if [[ -n "${WLEDPLAY_AIRPLAY_INTERFACE:-}" ]]; then
  echo "AirPlay/mDNS interface: ${WLEDPLAY_AIRPLAY_INTERFACE}"
fi
if [[ -n "$OFFSET" ]]; then
  echo "Shairport latency offset: ${OFFSET}s"
fi

rm -f "$META_PIPE"
mkfifo "$META_PIPE"
echo "Metadata pipe: ${META_PIPE}"

"$SHAIRPORT_BIN" -c "$TMP_CONF" --service-type=airplay2 -o stdout | \
  "$WLEDPLAY_BIN" --config "$CONFIG" --input stdin --no-realtime \
    --pcm-rate 48000 --pcm-channels 2 --pcm-format s16le \
    --metadata-pipe "$META_PIPE"
