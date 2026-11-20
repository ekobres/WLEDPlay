#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$(mktemp -d /tmp/wledplay-entry.XXXXXX)"
trap 'rm -rf "$fixture"' EXIT

make_socket() {
  python3 -c 'import socket, sys; sock = socket.socket(socket.AF_UNIX); sock.bind(sys.argv[1])' "$1"
}

eval "$(sed -n '/^start_mdns() {/,/^}/p' "$ROOT/docker/entrypoint.sh" | sed \
  -e 's@/host/run@__HOST_RUN__@g' \
  -e "s@/run/dbus@$fixture/run/dbus@g" \
  -e "s@/run/avahi-daemon@$fixture/run/avahi-daemon@g" \
  -e "s@__HOST_RUN__@$fixture/host/run@g")"

mkdir -p "$fixture/run/dbus"
make_socket "$fixture/run/dbus/system_bus_socket"
ENABLE_AVAHI=0
WLEDPLAY_AVAHI_MODE=auto
start_mdns
[[ -S "$fixture/run/dbus/system_bus_socket" ]]
[[ ! -L "$fixture/run/dbus/system_bus_socket" ]]
[[ "$DBUS_SYSTEM_BUS_ADDRESS" == "unix:path=$fixture/run/dbus/system_bus_socket" ]]

rm "$fixture/run/dbus/system_bus_socket"
mkdir -p "$fixture/host/run/dbus" "$fixture/host/run/avahi-daemon"
make_socket "$fixture/host/run/dbus/system_bus_socket"
make_socket "$fixture/host/run/avahi-daemon/socket"
ENABLE_AVAHI=""
WLEDPLAY_AVAHI_MODE=auto
start_mdns
[[ -L "$fixture/run/dbus/system_bus_socket" ]]
[[ -L "$fixture/run/avahi-daemon/socket" ]]

echo "Entrypoint host Avahi checks passed."