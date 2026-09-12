#!/usr/bin/env bash
# Phase 3: Shairport Sync (AirPlay 1) → WLEDPlay → WLED
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SHAIRPORT_BIN="${SHAIRPORT_BIN:-shairport-sync}"
WLEDPLAY_BIN="${WLEDPLAY_BIN:-$ROOT/target/release/wledplay}"
CONFIG="${WLEDPLAY_CONFIG:-$ROOT/config/config.yaml}"
SHAIRPORT_CONF="${SHAIRPORT_CONF:-$ROOT/config/shairport-sync.conf}"
AIRPLAY_NAME="${WLEDPLAY_AIRPLAY_NAME:-WLEDPlay}"

if [[ ! -x "$WLEDPLAY_BIN" ]]; then
  echo "Building wledplay..."
  env -u CARGO_TARGET_DIR cargo build --release
  WLEDPLAY_BIN="$ROOT/target/release/wledplay"
fi

if ! command -v "$SHAIRPORT_BIN" >/dev/null 2>&1; then
  echo "shairport-sync not found."
  echo "Install AirPlay 1 (Homebrew includes the stdout backend):"
  echo "  brew install shairport-sync"
  exit 1
fi

if ! "$SHAIRPORT_BIN" -V 2>/dev/null | grep -q stdout; then
  echo "This shairport-sync build has no stdout backend (need --with-stdout)."
  echo "Homebrew's formula includes it. Check: $SHAIRPORT_BIN -V"
  exit 1
fi

TMP_CONF="$(mktemp -t wledplay-shairport.XXXXXX.conf)"
cleanup() {
  rm -f "$TMP_CONF"
  if [[ -n "${SHAIRPORT_PID:-}" ]]; then
    kill "$SHAIRPORT_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

sed "s/name = \"WLEDPlay\"/name = \"${AIRPLAY_NAME}\"/" "$SHAIRPORT_CONF" > "$TMP_CONF"
if [[ -n "${WLEDPLAY_INTERFACE:-}" ]]; then
  python3 - "$TMP_CONF" "$WLEDPLAY_INTERFACE" <<'PY'
import pathlib, re, sys
p, iface = pathlib.Path(sys.argv[1]), sys.argv[2]
text = p.read_text()
if re.search(r"(?m)^[ \t]*//?[ \t]*interface\s*=", text) or re.search(r"(?m)^[ \t]*interface\s*=", text):
    text = re.sub(r"(?m)^[ \t]*//[ \t]*interface\s*=.*$", f'  interface = "{iface}";', text)
    text = re.sub(r"(?m)^[ \t]*interface\s*=.*$", f'  interface = "{iface}";', text)
else:
    text = text.replace("general = {", f'general = {{\n  interface = "{iface}";', 1)
p.write_text(text)
PY
fi

echo "Advertising AirPlay receiver: ${AIRPLAY_NAME}"
echo "WLEDPlay config: ${CONFIG}"
echo "Select this destination in Apple Music / Control Center, then play."

# Homebrew shairport-sync 5.5.x always binds classic AirPlay on TCP 5000
# (CLI --port and general.port are overwritten). macOS AirPlay Receiver
# uses that port — turn it off in System Settings → General → AirDrop & Handoff.
if command -v lsof >/dev/null 2>&1 && lsof -nP -iTCP:5000 -sTCP:LISTEN >/dev/null 2>&1; then
  echo
  echo "TCP port 5000 is already in use (usually macOS Control Center AirPlay Receiver)."
  echo "Shairport Sync 5.5 cannot use another port for AirPlay 1."
  echo
  echo "Turn off AirPlay Receiver:"
  echo "  System Settings → General → AirDrop & Handoff → AirPlay Receiver → Off"
  echo
  echo "Then re-run: $0"
  exit 1
fi

# Shairport writes PCM in real time; do not pace again.
IFACE_ARGS=()
if [[ -n "${WLEDPLAY_INTERFACE:-}" ]]; then
  IFACE_ARGS=(--interface "$WLEDPLAY_INTERFACE")
  echo "Interface: ${WLEDPLAY_INTERFACE}"
fi

"$SHAIRPORT_BIN" -c "$TMP_CONF" -o stdout | \
  "$WLEDPLAY_BIN" --config "$CONFIG" --input stdin --no-realtime "${IFACE_ARGS[@]}"
