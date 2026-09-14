#!/bin/sh
set -eu

HOST_CFG="/host-config"
CFG_YAML="/etc/wledplay/config.yaml"
if [ -f "$HOST_CFG/config.yaml" ]; then
  CFG_YAML="$HOST_CFG/config.yaml"
fi
SHAIRPORT_SRC="/etc/shairport-sync.conf"
if [ -f "$HOST_CFG/shairport-sync.conf" ]; then
  SHAIRPORT_SRC="$HOST_CFG/shairport-sync.conf"
elif [ -f "$HOST_CFG/shairport-sync-example.conf" ]; then
  SHAIRPORT_SRC="$HOST_CFG/shairport-sync-example.conf"
fi
yaml_scalar() {
  key=$1
  [ -f "$CFG_YAML" ] || return 0
  val=$(sed -n "s/^[[:space:]]*${key}:[[:space:]]*\"\\(.*\\)\"[[:space:]]*$/\\1/p" "$CFG_YAML" 2>/dev/null | head -1)
  if [ -n "$val" ]; then
    printf '%s' "$val"
    return
  fi
  sed -n "s/^[[:space:]]*${key}:[[:space:]]*\\([^#[:space:]\"].*\\)$/\\1/p" "$CFG_YAML" 2>/dev/null | head -1 | tr -d '[:space:]'
}

pick_env() {
  var=$1
  key=$2
  eval "cur=\${$var:-}"
  if [ -z "$cur" ]; then
    cur=$(yaml_scalar "$key")
    eval "$var=\$cur"
  fi
}

pick_env WLEDPLAY_AIRPLAY_NAME name
WLEDPLAY_AIRPLAY_NAME="${WLEDPLAY_AIRPLAY_NAME:-WLEDPlay}"
WLEDPLAY_MULTICAST_ADDRESS="${WLEDPLAY_MULTICAST_ADDRESS:-239.0.0.1}"
WLEDPLAY_PORT="${WLEDPLAY_PORT:-11988}"
WLEDPLAY_LOG_LEVEL="${WLEDPLAY_LOG_LEVEL:-info}"
WLEDPLAY_PCM_RATE="${WLEDPLAY_PCM_RATE:-48000}"
WLEDPLAY_PCM_CHANNELS="${WLEDPLAY_PCM_CHANNELS:-2}"
WLEDPLAY_PCM_FORMAT="${WLEDPLAY_PCM_FORMAT:-s16le}"
WLEDPLAY_DESTINATION="${WLEDPLAY_DESTINATION:-}"
pick_env WLEDPLAY_HTTP_INTERFACE http
pick_env WLEDPLAY_AIRPLAY_INTERFACE airplay
pick_env WLEDPLAY_UNICAST_INTERFACE unicast
pick_env WLEDPLAY_MULTICAST_INTERFACE multicast
export WLEDPLAY_HTTP_INTERFACE WLEDPLAY_AIRPLAY_INTERFACE
export WLEDPLAY_UNICAST_INTERFACE WLEDPLAY_MULTICAST_INTERFACE
WLEDPLAY_SHAIRPORT_LATENCY_OFFSET="${WLEDPLAY_SHAIRPORT_LATENCY_OFFSET:-}"
if [ -z "$WLEDPLAY_SHAIRPORT_LATENCY_OFFSET" ]; then
  WLEDPLAY_SHAIRPORT_LATENCY_OFFSET="$(sed -n 's/^[[:space:]]*shairport_latency_offset_s:[[:space:]]*//p' "$CFG_YAML" 2>/dev/null | head -1)"
fi
ENABLE_AVAHI="${ENABLE_AVAHI:-1}"
PCM_FIFO="/run/wledplay.pcm"
META_FIFO="${WLEDPLAY_METADATA_PIPE:-/run/wledplay.meta}"

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
  if [ -n "$WLEDPLAY_AIRPLAY_INTERFACE" ]; then
    if grep -q '^#*allow-interfaces=' "$avahi_conf"; then
      sed -i "s|^#*allow-interfaces=.*|allow-interfaces=${WLEDPLAY_AIRPLAY_INTERFACE}|" "$avahi_conf"
    else
      sed -i "/^\\[server\\]/a allow-interfaces=${WLEDPLAY_AIRPLAY_INTERFACE}" "$avahi_conf"
    fi
    echo "Avahi allow-interfaces: ${WLEDPLAY_AIRPLAY_INTERFACE}"
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

echo "Starting nqptp..."
nqptp &
NQPTP_PID=$!

sed "s/WLEDPlay/${WLEDPLAY_AIRPLAY_NAME}/g" "$SHAIRPORT_SRC" > /tmp/shairport-sync.conf
if [ -n "$WLEDPLAY_AIRPLAY_INTERFACE" ]; then
  if grep -q 'interface =' /tmp/shairport-sync.conf; then
    sed -i "s|^[[:space:]]*//[[:space:]]*interface = .*|  interface = \"${WLEDPLAY_AIRPLAY_INTERFACE}\";|" /tmp/shairport-sync.conf
    sed -i "s|^[[:space:]]*interface = .*|  interface = \"${WLEDPLAY_AIRPLAY_INTERFACE}\";|" /tmp/shairport-sync.conf
  else
    sed -i "/^general = {/a\\
  interface = \"${WLEDPLAY_AIRPLAY_INTERFACE}\";
" /tmp/shairport-sync.conf
  fi
  echo "Shairport interface: ${WLEDPLAY_AIRPLAY_INTERFACE}"
fi
if [ -n "$WLEDPLAY_SHAIRPORT_LATENCY_OFFSET" ]; then
  sed -i "s|^[[:space:]]*audio_backend_latency_offset_in_seconds = .*|  audio_backend_latency_offset_in_seconds = ${WLEDPLAY_SHAIRPORT_LATENCY_OFFSET};|" /tmp/shairport-sync.conf
  echo "Shairport latency offset: ${WLEDPLAY_SHAIRPORT_LATENCY_OFFSET}s"
fi
sed -i "s|^[[:space:]]*pipe_name = .*|  pipe_name = \"${META_FIFO}\";|" /tmp/shairport-sync.conf

rm -f "$PCM_FIFO" "$META_FIFO"
mkfifo "$PCM_FIFO"
mkfifo "$META_FIFO"

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

echo "Starting wledplay..."
# shellcheck disable=SC2086
/wledplay \
  --config "$CFG_YAML" \
  --input stdin \
  --no-realtime \
  --multicast "${WLEDPLAY_MULTICAST_ADDRESS}" \
  --port "${WLEDPLAY_PORT}" \
  --pcm-rate "${WLEDPLAY_PCM_RATE}" \
  --pcm-channels "${WLEDPLAY_PCM_CHANNELS}" \
  --pcm-format "${WLEDPLAY_PCM_FORMAT}" \
  $DEST_ARGS \
  --metadata-pipe "$META_FIFO" \
  < "$PCM_FIFO" &
WLEDPLAY_PID=$!

echo "Starting shairport-sync -> FIFO -> wledplay..."
shairport-sync -c /tmp/shairport-sync.conf > "$PCM_FIFO" &
SHAIRPORT_PID=$!

echo "WLEDPlay stack running (AirPlay: ${WLEDPLAY_AIRPLAY_NAME}, WLED: ${WLEDPLAY_MULTICAST_ADDRESS}:${WLEDPLAY_PORT})"
wait "$WLEDPLAY_PID" || true
shutdown
