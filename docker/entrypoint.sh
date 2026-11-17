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
# Read one `key: value` from a top-level YAML section. Controller rows also use
# `name` and `unicast`, so an unscoped search would treat `unicast: false` as
# the unicast NIC and the process would exit at startup.
yaml_scalar() {
  key=$1
  section=$2
  [ -f "$CFG_YAML" ] || return 0
  awk -v section="$section" -v key="$key" '
    $0 ~ "^" section ":" { inside = 1; next }
    inside && $0 ~ "^[^[:space:]#]" { exit }
    inside {
      line = $0
      sub(/^[[:space:]]+/, "", line)
      sub(/[[:space:]]+$/, "", line)
      if (line !~ "^" key ":") next
      sub("^" key ":[[:space:]]*", "", line)
      if (line ~ /^".*"$/) {
        sub(/^"/, "", line)
        sub(/"$/, "", line)
      }
      if (line != "") print line
      exit
    }
  ' "$CFG_YAML"
}

pick_env() {
  var=$1
  key=$2
  section=$3
  eval "cur=\${$var:-}"
  if [ -z "$cur" ]; then
    cur=$(yaml_scalar "$key" "$section")
    eval "$var=\$cur"
  fi
}

pick_env WLEDPLAY_AIRPLAY_NAME name airplay
WLEDPLAY_AIRPLAY_NAME="${WLEDPLAY_AIRPLAY_NAME:-WLEDPlay}"
WLEDPLAY_MULTICAST_ADDRESS="${WLEDPLAY_MULTICAST_ADDRESS:-239.0.0.1}"
WLEDPLAY_PORT="${WLEDPLAY_PORT:-11988}"
WLEDPLAY_LOG_LEVEL="${WLEDPLAY_LOG_LEVEL:-info}"
WLEDPLAY_LOG_DIR="${WLEDPLAY_LOG_DIR:-/var/log/wledplay}"
export WLEDPLAY_LOG_DIR
WLEDPLAY_PCM_RATE="${WLEDPLAY_PCM_RATE:-48000}"
WLEDPLAY_PCM_CHANNELS="${WLEDPLAY_PCM_CHANNELS:-2}"
WLEDPLAY_PCM_FORMAT="${WLEDPLAY_PCM_FORMAT:-s16le}"
WLEDPLAY_DESTINATION="${WLEDPLAY_DESTINATION:-}"
pick_env WLEDPLAY_HTTP_INTERFACE http network
pick_env WLEDPLAY_AIRPLAY_INTERFACE airplay network
pick_env WLEDPLAY_UNICAST_INTERFACE unicast network
pick_env WLEDPLAY_MULTICAST_INTERFACE multicast network
export WLEDPLAY_HTTP_INTERFACE WLEDPLAY_AIRPLAY_INTERFACE
export WLEDPLAY_UNICAST_INTERFACE WLEDPLAY_MULTICAST_INTERFACE
WLEDPLAY_SHAIRPORT_LATENCY_OFFSET="${WLEDPLAY_SHAIRPORT_LATENCY_OFFSET:-}"
if [ -z "$WLEDPLAY_SHAIRPORT_LATENCY_OFFSET" ]; then
  WLEDPLAY_SHAIRPORT_LATENCY_OFFSET="$(sed -n 's/^[[:space:]]*shairport_latency_offset_s:[[:space:]]*//p' "$CFG_YAML" 2>/dev/null | head -1)"
fi
ENABLE_AVAHI="${ENABLE_AVAHI:-1}"
WLEDPLAY_IPV4_ONLY="${WLEDPLAY_IPV4_ONLY:-0}"
PCM_FIFO="/run/wledplay.pcm"
META_FIFO="${WLEDPLAY_METADATA_PIPE:-/run/wledplay.meta}"
SIDECAR_PID=""
NQPTP_PID=""
NQPTP_WATCH_PID=""
SHAIRPORT_PID=""
WLEDPLAY_PID=""
ROTATE_PID=""
TEE_PIDS=""

wait_for() {
  path=$1
  tries=${2:-50}
  pause=${3:-0.1}
  i=0
  while [ "$i" -lt "$tries" ]; do
    [ -e "$path" ] && return 0
    sleep "$pause"
    i=$((i + 1))
  done
  return 1
}

wait_for_tcp() {
  host=$1
  port=$2
  tries=${3:-50}
  if python3 - "$host" "$port" "$tries" <<'PY'
import socket
import sys
import time

host, port, tries = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
for attempt in range(tries):
    try:
        sock = socket.create_connection((host, port), 0.2)
        sock.close()
        raise SystemExit(0)
    except OSError:
        if attempt + 1 < tries:
            time.sleep(0.1)
raise SystemExit(1)
PY
  then
    return 0
  fi
  return 1
}

watch_nqptp() {
  if wait_for /dev/shm/nqptp 600 0.5; then
    echo "nqptp clock is ready"
    return 0
  fi
  echo "warning: nqptp clock file /dev/shm/nqptp is still missing"
}

# Each process log is also copied to the container console. The fifo is opened
# read-write so tee does not block waiting for the writer. stdbuf flushes each
# line so the Logs card sees it immediately. Files stay under 8MB.
setup_logs() {
  mkdir -p "$WLEDPLAY_LOG_DIR"
  exec 3>&1
  rm -f "$WLEDPLAY_LOG_DIR/console.fifo"
  mkfifo "$WLEDPLAY_LOG_DIR/console.fifo"
  stdbuf -oL tee -a "$WLEDPLAY_LOG_DIR/console.log" <>"$WLEDPLAY_LOG_DIR/console.fifo" >&3 &
  TEE_PIDS="$TEE_PIDS $!"
  exec >"$WLEDPLAY_LOG_DIR/console.fifo" 2>&1
  rotate_logs &
  ROTATE_PID=$!
}

start_log_tee() {
  name=$1
  fifo="$WLEDPLAY_LOG_DIR/$name.fifo"
  rm -f "$fifo"
  mkfifo "$fifo"
  stdbuf -oL tee -a "$WLEDPLAY_LOG_DIR/$name.log" <>"$fifo" &
  TEE_PIDS="$TEE_PIDS $!"
}

rotate_logs() {
  while true; do
    sleep 60
    for name in wledplay shairport nqptp console; do
      file="$WLEDPLAY_LOG_DIR/$name.log"
      [ -f "$file" ] || continue
      size=$(wc -c < "$file" 2>/dev/null || echo 0)
      if [ "$size" -gt 8388608 ]; then
        tail -c 2097152 "$file" > "$file.part"
        cat "$file.part" > "$file"
        rm -f "$file.part"
      fi
    done
  done
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
  avahi_ifaces=""
  if [ -n "$WLEDPLAY_AIRPLAY_INTERFACE" ]; then
    avahi_ifaces="$WLEDPLAY_AIRPLAY_INTERFACE"
  fi
  if [ -n "$WLEDPLAY_UNICAST_INTERFACE" ] && [ "$WLEDPLAY_UNICAST_INTERFACE" != "$WLEDPLAY_AIRPLAY_INTERFACE" ]; then
    if [ -n "$avahi_ifaces" ]; then
      avahi_ifaces="${avahi_ifaces},${WLEDPLAY_UNICAST_INTERFACE}"
    else
      avahi_ifaces="$WLEDPLAY_UNICAST_INTERFACE"
    fi
  fi
  if [ -n "$avahi_ifaces" ]; then
    if grep -q '^#*allow-interfaces=' "$avahi_conf"; then
      sed -i "s|^#*allow-interfaces=.*|allow-interfaces=${avahi_ifaces}|" "$avahi_conf"
    else
      sed -i "/^\\[server\\]/a allow-interfaces=${avahi_ifaces}" "$avahi_conf"
    fi
    echo "Avahi allow-interfaces: ${avahi_ifaces}"
  fi
  if [ "$WLEDPLAY_IPV4_ONLY" = "1" ]; then
    sed -i 's/^use-ipv6=yes/use-ipv6=no/' "$avahi_conf"
    echo "Avahi IPv6 disabled (mDNS only; timing peers are unchanged)"
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

start_autojoin() {
  storage="${PYATV_STORAGE:-/host-config/pyatv}"
  case "$storage" in
    *.json|*.conf) mkdir -p "$(dirname "$storage")" ;;
    *) mkdir -p "$storage" ;;
  esac
  export PYATV_STORAGE="$storage"
  export WLEDPLAY_AUTOJOIN_URL="${WLEDPLAY_AUTOJOIN_URL:-http://127.0.0.1:8091}"
  export WLEDPLAY_AIRPLAY_NAME
  echo "Starting Auto Join sidecar..."
  python3 /usr/local/bin/autojoin_sidecar.py &
  SIDECAR_PID=$!
}

shutdown() {
  echo "Shutting down WLEDPlay stack..."
  kill "$WLEDPLAY_PID" 2>/dev/null || true
  kill "$SIDECAR_PID" 2>/dev/null || true
  kill "$SHAIRPORT_PID" 2>/dev/null || true
  kill "$NQPTP_PID" 2>/dev/null || true
  kill "$NQPTP_WATCH_PID" 2>/dev/null || true
  exec >&3 2>&3 || true
  # shellcheck disable=SC2086
  kill $TEE_PIDS "$ROTATE_PID" 2>/dev/null || true
  if [ -f /run/avahi-daemon/pid ]; then
    kill "$(cat /run/avahi-daemon/pid)" 2>/dev/null || true
  fi
  if [ -f /run/dbus/pid ]; then
    kill "$(cat /run/dbus/pid)" 2>/dev/null || true
  fi
  wait || true
}
trap shutdown INT TERM

setup_logs
start_mdns

echo "Starting nqptp..."
start_log_tee nqptp
nqptp > "$WLEDPLAY_LOG_DIR/nqptp.fifo" 2>&1 &
NQPTP_PID=$!
# AirPlay 2 does not start the stdout backend until this clock exists.
if ! wait_for /dev/shm/nqptp 50; then
  echo "warning: nqptp has not created /dev/shm/nqptp yet"
  watch_nqptp &
  NQPTP_WATCH_PID=$!
fi
if ! kill -0 "$NQPTP_PID" 2>/dev/null; then
  echo "fatal: nqptp exited. AirPlay 2 needs UDP 319/320."
  exit 1
fi

sed "s/WLEDPlay/${WLEDPLAY_AIRPLAY_NAME}/g" "$SHAIRPORT_SRC" > /tmp/shairport-sync.conf
# stdout is the PCM pipe. Daemonizing closes that fd and leaves metadata working.
if grep -Eq '^[[:space:]]*(//[[:space:]]*)?daemonise[[:space:]]*=' /tmp/shairport-sync.conf; then
  sed -i 's|^[[:space:]]*//[[:space:]]*daemonise = .*|  daemonise = "no";|' /tmp/shairport-sync.conf
  sed -i 's|^[[:space:]]*daemonise = .*|  daemonise = "no";|' /tmp/shairport-sync.conf
else
  sed -i '/^general = {/a\
  daemonise = "no";
' /tmp/shairport-sync.conf
fi
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

echo "Starting Auto Join..."
start_autojoin
autojoin_url="${WLEDPLAY_AUTOJOIN_URL:-http://127.0.0.1:8091}"
autojoin_host=$(printf '%s' "$autojoin_url" | sed -E 's#^[a-zA-Z]+://([^:/]+).*#\1#')
autojoin_port=$(printf '%s' "$autojoin_url" | sed -E 's#^.*:([0-9]+).*#\1#')
case "$autojoin_port" in
  ''|*[!0-9]*) autojoin_port=8091 ;;
esac
if ! wait_for_tcp "$autojoin_host" "$autojoin_port" 50; then
  echo "warning: Auto Join sidecar is not accepting connections yet"
fi

echo "WLEDPlay stack running (AirPlay: ${WLEDPLAY_AIRPLAY_NAME}, WLED: ${WLEDPLAY_MULTICAST_ADDRESS}:${WLEDPLAY_PORT})"
echo "Starting wledplay..."
start_log_tee wledplay
# shellcheck disable=SC2086
/wledplay \
  --config "$CFG_YAML" \
  --input "$PCM_FIFO" \
  --no-realtime \
  --multicast "${WLEDPLAY_MULTICAST_ADDRESS}" \
  --port "${WLEDPLAY_PORT}" \
  --pcm-rate "${WLEDPLAY_PCM_RATE}" \
  --pcm-channels "${WLEDPLAY_PCM_CHANNELS}" \
  --pcm-format "${WLEDPLAY_PCM_FORMAT}" \
  $DEST_ARGS \
  --metadata-pipe "$META_FIFO" > "$WLEDPLAY_LOG_DIR/wledplay.fifo" 2>&1 &
WLEDPLAY_PID=$!

echo "Starting shairport-sync -> FIFO -> wledplay..."
start_log_tee shairport
shairport-sync -c /tmp/shairport-sync.conf > "$PCM_FIFO" 2>"$WLEDPLAY_LOG_DIR/shairport.fifo" &
SHAIRPORT_PID=$!

# Compose restart: unless-stopped brings the container back after this process exits.
# wledplay exiting is the orderly restart path. Shairport, NQPTP, or the sidecar
# dying is a failed child: stop the rest and exit non-zero.
child_failed=0
while true; do
  if ! kill -0 "$WLEDPLAY_PID" 2>/dev/null; then
    break
  fi
  if ! kill -0 "$SHAIRPORT_PID" 2>/dev/null \
    || ! kill -0 "$NQPTP_PID" 2>/dev/null \
    || ! kill -0 "$SIDECAR_PID" 2>/dev/null; then
    echo "A stack process exited; stopping the container so it can restart."
    child_failed=1
    break
  fi
  sleep 1
done
shutdown
exit "$child_failed"
