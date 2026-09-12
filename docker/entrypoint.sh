#!/bin/sh
set -eu

WLEDPLAY_AIRPLAY_NAME="${WLEDPLAY_AIRPLAY_NAME:-WLEDPlay}"
WLEDPLAY_MULTICAST_ADDRESS="${WLEDPLAY_MULTICAST_ADDRESS:-239.0.0.1}"
WLEDPLAY_PORT="${WLEDPLAY_PORT:-11988}"
WLEDPLAY_LOG_LEVEL="${WLEDPLAY_LOG_LEVEL:-info}"
ENABLE_AIRPLAY2="${ENABLE_AIRPLAY2:-1}"
if [ "$ENABLE_AIRPLAY2" = "1" ]; then
  WLEDPLAY_PCM_RATE="${WLEDPLAY_PCM_RATE:-48000}"
else
  WLEDPLAY_PCM_RATE="${WLEDPLAY_PCM_RATE:-44100}"
fi
WLEDPLAY_PCM_CHANNELS="${WLEDPLAY_PCM_CHANNELS:-2}"
WLEDPLAY_PCM_FORMAT="${WLEDPLAY_PCM_FORMAT:-s16le}"
WLEDPLAY_DESTINATION="${WLEDPLAY_DESTINATION:-}"
WLEDPLAY_INTERFACE="${WLEDPLAY_INTERFACE:-}"
PCM_FIFO="/run/wledplay.pcm"

shutdown() {
  echo "Shutting down WLEDPlay stack..."
  kill "$WLEDPLAY_PID" 2>/dev/null || true
  kill "$SHAIRPORT_PID" 2>/dev/null || true
  kill "$NQPTP_PID" 2>/dev/null || true
  wait || true
}
trap shutdown INT TERM

if [ "$ENABLE_AIRPLAY2" = "1" ]; then
  echo "Starting nqptp..."
  nqptp &
  NQPTP_PID=$!
fi

sed "s/WLEDPlay/${WLEDPLAY_AIRPLAY_NAME}/g" /etc/shairport-sync.conf > /tmp/shairport-sync.conf
if [ -n "$WLEDPLAY_INTERFACE" ]; then
  if grep -q 'interface =' /tmp/shairport-sync.conf; then
    sed -i "s|^[[:space:]]*//[[:space:]]*interface = .*|  interface = \"${WLEDPLAY_INTERFACE}\";|" /tmp/shairport-sync.conf
    sed -i "s|^[[:space:]]*interface = .*|  interface = \"${WLEDPLAY_INTERFACE}\";|" /tmp/shairport-sync.conf
  else
    sed -i "/^general = {/a\\
  interface = \"${WLEDPLAY_INTERFACE}\";
" /tmp/shairport-sync.conf
  fi
  echo "Shairport interface: ${WLEDPLAY_INTERFACE}"
fi

rm -f "$PCM_FIFO"
mkfifo "$PCM_FIFO"

DEST_ARGS=""
if [ -n "$WLEDPLAY_DESTINATION" ]; then
  OLDIFS=$IFS
  IFS=,
  for dest in $WLEDPLAY_DESTINATION; do
    dest=$(echo "$dest" | tr -d ' ')
    [ -n "$dest" ] && DEST_ARGS="$DEST_ARGS --destination $dest"
  done
  IFS=$OLDIFS
fi

IFACE_ARGS=""
if [ -n "$WLEDPLAY_INTERFACE" ]; then
  IFACE_ARGS="--interface ${WLEDPLAY_INTERFACE}"
fi

echo "Starting wledplay..."
# shellcheck disable=SC2086
/wledplay \
  --config /etc/wledplay/config.yaml \
  --input stdin \
  --no-realtime \
  --multicast "${WLEDPLAY_MULTICAST_ADDRESS}" \
  --port "${WLEDPLAY_PORT}" \
  --pcm-rate "${WLEDPLAY_PCM_RATE}" \
  --pcm-channels "${WLEDPLAY_PCM_CHANNELS}" \
  --pcm-format "${WLEDPLAY_PCM_FORMAT}" \
  $DEST_ARGS \
  $IFACE_ARGS \
  < "$PCM_FIFO" &
WLEDPLAY_PID=$!

echo "Starting shairport-sync -> FIFO -> wledplay..."
shairport-sync -c /tmp/shairport-sync.conf > "$PCM_FIFO" &
SHAIRPORT_PID=$!

echo "WLEDPlay stack running (AirPlay: ${WLEDPLAY_AIRPLAY_NAME}, WLED: ${WLEDPLAY_MULTICAST_ADDRESS}:${WLEDPLAY_PORT})"
wait "$SHAIRPORT_PID"
