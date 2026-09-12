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
ENABLE_AVAHI="${ENABLE_AVAHI:-1}"
PCM_FIFO="/run/wledplay.pcm"

wait_for() {
  path=$1
  tries=${2:-50}
  i=0
  while [ "$i" -lt "$tries" ]; do
    [ -e "$path" ] && return 0
    sleep 0.1
    i=$((i + 1))
  done
  return 1
}

start_mdns() {
  if [ "$ENABLE_AVAHI" != "1" ]; then
    echo "ENABLE_AVAHI=0: expecting host D-Bus/Avahi (mount /run/dbus)"
    return 0
  fi

  mkdir -p /run/dbus /run/avahi-daemon
  rm -f /run/dbus/pid /run/dbus/dbus.pid /run/avahi-daemon/pid

  echo "Starting dbus..."
  dbus-uuidgen --ensure
  dbus-daemon --system
  if ! wait_for /run/dbus/system_bus_socket 50; then
    echo "fatal: dbus-daemon did not create /run/dbus/system_bus_socket"
    exit 1
  fi

  avahi_conf=/tmp/avahi-daemon.conf
  cp /etc/avahi/avahi-daemon.conf "$avahi_conf"
  if [ -n "$WLEDPLAY_INTERFACE" ]; then
    if grep -q '^#*allow-interfaces=' "$avahi_conf"; then
      sed -i "s|^#*allow-interfaces=.*|allow-interfaces=${WLEDPLAY_INTERFACE}|" "$avahi_conf"
    else
      sed -i "/^\\[server\\]/a allow-interfaces=${WLEDPLAY_INTERFACE}" "$avahi_conf"
    fi
    echo "Avahi allow-interfaces: ${WLEDPLAY_INTERFACE}"
  fi

  echo "Starting avahi-daemon..."
  avahi-daemon --daemonize --no-chroot --file="$avahi_conf"
  if ! wait_for /run/avahi-daemon/pid 50; then
    echo "fatal: avahi-daemon did not start."
    echo "With network_mode: host, a host avahi-daemon already bound to UDP 5353 will block this."
    echo "Stop the host daemon, or set ENABLE_AVAHI=0 and mount /run/dbus:/run/dbus"
    exit 1
  fi
  echo "Avahi is running (pid $(cat /run/avahi-daemon/pid))"
}

shutdown() {
  echo "Shutting down WLEDPlay stack..."
  kill "$WLEDPLAY_PID" 2>/dev/null || true
  kill "$SHAIRPORT_PID" 2>/dev/null || true
  kill "$NQPTP_PID" 2>/dev/null || true
  if [ -f /run/avahi-daemon/pid ]; then
    kill "$(cat /run/avahi-daemon/pid)" 2>/dev/null || true
  fi
  if [ -f /run/dbus/pid ]; then
    kill "$(cat /run/dbus/pid)" 2>/dev/null || true
  fi
  wait || true
}
trap shutdown INT TERM

start_mdns

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
