# WLEDPlay

WLEDPlay is an AirPlay 2 receiver that turns the music into WLED Audio Sync. It analyzes the decoded PCM and sends Audio Sync V2 packets to one or more WLED 16.x controllers. The controllers render the effects. WLEDPlay does not play the audio itself.

The lights follow the stream, not a microphone or a line-in cable:

- There is no microphone and no hard-wired line-in. Effects follow the AirPlay stream.
- Speaker volume does not matter. The analysis uses the music, not how loud the room is.
- Room noise does not trigger effects or wash them out.
- The lights are driven from a digital transform of the decoded PCM, the same kind of 16-band analysis WLED uses.
- Send delay and Shairport lead can be trimmed until the lights match the speakers. See [`docs/SYNC_MEASUREMENT.md`](docs/SYNC_MEASUREMENT.md).

This requires AirPlay 2 so WLEDPlay can join a group with HomePods, Apple TVs, and other AirPlay 2 speakers. AirPlay 1 is unsupported because it cannot share that presentation clock.

## Quick start

WLEDPlay runs on a Linux host (Pi, NUC, LXC, or Docker). NQPTP needs UDP 319 and 320, which macOS already uses, so the receiver cannot run on a Mac. Docker Desktop for Mac cannot advertise AirPlay on the LAN because `network_mode: host` is a Linux feature.

```bash
./scripts/ensure-config.sh
WLEDPLAY_DESTINATION=192.0.2.10 \
WLEDPLAY_AIRPLAY_INTERFACE=eth1 \
WLEDPLAY_UNICAST_INTERFACE=eth2 \
WLEDPLAY_MULTICAST_INTERFACE=eth2 \
docker compose up -d --build
```

`192.0.2.10` is a documentation address. Substitute your WLED controller. Leave a NIC variable unset when you do not need to pin that traffic. Compose builds the image on this machine and tags it `ghcr.io/wledplay/wledplay`. That name is not a published registry image; do not `docker pull` it.

Without Docker, on a Linux host:

```bash
./scripts/build-airplay.sh
./scripts/run-airplay.sh
```

### WLED

1. Flash official WLED **16.x** (for example v16.0.1 or newer).
2. Enable **Audio Reactive**.
3. Set **Sync Mode** to **Receive**.
4. Use UDP port **11988**, unless you changed `WLEDPLAY_PORT`.

Open the operator UI at `http://<host>:8080`. Group **WLEDPlay** with a HomePod or Apple TV and compare the lights to the speakers ([`docs/SYNC_MEASUREMENT.md`](docs/SYNC_MEASUREMENT.md)). AirPlay PCM is pinned to **48000 Hz S16LE**.

### When the lights stay idle

WLEDPlay sends **UDP** to the controller. If WLEDPlay and the controller are on different VLANs, the WLED web UI can load while **UDP 11988** is blocked. WLED then shows **Audio Source: UDP sound sync - idle**.

```bash
python3 scripts/verify_wled_sync.py 192.0.2.10
```

Run WLEDPlay on a host in the **same VLAN as WLED**, or allow **UDP 11988** to the controller. Multicast (`239.0.0.1`) does not cross VLANs. Unicast to the controller address from the same VLAN is the reliable path. On a multi-homed host, set `network.unicast` and `network.multicast` (or `WLEDPLAY_UNICAST_INTERFACE` and `WLEDPLAY_MULTICAST_INTERFACE`) to the WLED VLAN NIC so `239.0.0.1` is not sent out the management default route. IGMP snooping may keep that group off the router trunk if no receiver on that path has joined.

Example: allow `192.0.2.10:11988/udp` from the WLEDPlay host. Substitute your controller address.

## Using WLEDPlay

The operator UI is the way to change a running player.

Live, on the next packet:

- Send delay, AGC, multicast on or off, unicast destinations, and Auto Join.

After **Restart stack**:

- Shairport lead, the AirPlay name, and the four NIC pins (HTTP, AirPlay/mDNS, unicast, multicast).

The UI listens on `0.0.0.0:8080` by default and does not authenticate. `PATCH /config` and `POST /restart` are open to anyone who can reach that port. Bind `network.http` or `WLEDPLAY_HTTP_INTERFACE` to a management NIC, or firewall TCP 8080. See [`SECURITY.md`](SECURITY.md).

### HTTP

- `GET /` — operator UI
- `GET /health` — liveness
- `GET /status` — JSON runtime state (default `:8080`)
- `GET /levels` — slim live meters (RMS, peak, frequency, 16 bands) for the UI poll
- `GET /artwork` — current cover art (404 if none)
- `GET /config` / `PATCH /config` — live settings
- `GET /interfaces` — usable IPv4 NICs
- `GET /airplay/devices` — AirPlay receivers from the last target snapshot
- `POST /airplay/rescan` — look up AirPlay speakers again and replace that list
- `GET /wled/controllers` — saved unicast rows merged with WLED controllers found on the network
- `POST /wled/rescan` — look up WLED controllers again
- `POST /restart` — orderly process restart

## Configuration

Copy the examples to gitignored local files and edit. Pulls update the examples only. They never overwrite local destinations, NIC pins, delay, or Shairport buffers.

```bash
./scripts/ensure-config.sh
```

That copies [`config/config-example.yaml`](config/config-example.yaml) to `config/config.yaml` and [`config/shairport-sync-example.conf`](config/shairport-sync-example.conf) to `config/shairport-sync.conf` when those files are missing. Docker Compose mounts `./config` and uses the local Shairport file if present, otherwise the example. Name, NIC pins, and Shairport lead are patched at start from yaml or env. Docker always creates the metadata FIFO at `WLEDPLAY_METADATA_PIPE` (default `/run/wledplay.meta`) and passes it to Shairport and `wledplay`. An empty `metadata.pipe` in yaml does not turn metadata off inside the container.

On a multi-homed host, pin each traffic type to a NIC (`ip -br addr`): HTTP (management UI), AirPlay/mDNS (user VLAN), unicast (WLED device IPs), and multicast (`239.0.0.1`, often a third IoT NIC). Empty means no pin. HTTP then uses `0.0.0.0:8080`. The container starts its own D-Bus and Avahi. If the host already owns UDP 5353, stop host `avahi-daemon` or set `ENABLE_AVAHI=0` and mount `/run/dbus`.

| Variable | Purpose |
|----------|---------|
| `WLEDPLAY_AIRPLAY_NAME` | Shairport Sync advertised name |
| `WLEDPLAY_MULTICAST_ADDRESS` | Default `239.0.0.1` |
| `WLEDPLAY_PORT` | Default `11988` |
| `WLEDPLAY_DESTINATION` | Comma-separated unicast WLED IPs (Docker) |
| `WLEDPLAY_HTTP_INTERFACE` | NIC for the HTTP UI (empty = `web.listen` / all interfaces) |
| `WLEDPLAY_AIRPLAY_INTERFACE` | NIC for Shairport. Avahi listens on this NIC |
| `WLEDPLAY_UNICAST_INTERFACE` | NIC for WLED unicast UDP. When this differs from the AirPlay NIC, Avahi listens on it too |
| `WLEDPLAY_MULTICAST_INTERFACE` | NIC for WLED multicast (`239.0.0.1`) |
| `WLEDPLAY_WEB_LISTEN` | HTTP bind address when HTTP NIC is unset (default `0.0.0.0`) |
| `WLEDPLAY_WEB_PORT` | HTTP port (default `8080`) |
| `WLEDPLAY_METADATA_PIPE` | Shairport metadata FIFO (Docker: `/run/wledplay.meta`) |
| `WLEDPLAY_SHAIRPORT_LATENCY_OFFSET` | Shairport `audio_backend_latency_offset_in_seconds` (e.g. `-1.2`) |
| `WLEDPLAY_MULTICAST_ENABLED` | `false` / `0` disables multicast send (unicast still works) |
| `ENABLE_AVAHI` | `1` (default) starts D-Bus + Avahi in the container; `0` uses host Avahi |
| `WLEDPLAY_IPV4_ONLY` | `1` sets Avahi `use-ipv6=no`. Does not change host IPv6 or Shairport's timing-peer list |
| `WLEDPLAY_SHAIRPORT_PATCH` | Build arg. `1` (default) applies `docker/shairport-mixed-stack.patch`; `0` builds stock 5.6.3 |
| `WLEDPLAY_NQPTP_PATCH` | Build arg. `1` (default) applies `docker/nqptp-timing-peers.patch`; `0` builds stock 1.2.9 |
| `WLEDPLAY_SYNC_OFFSET_MS` | Live send delay in milliseconds (`audio.sync_offset_ms`). Negative values clamp to `0` |
| `WLEDPLAY_LOG_LEVEL` | Log level |

## Docker

Requires **host networking** (mDNS, multicast, AirPlay 2 PTP).

Stack: `dbus` + `avahi` → `nqptp` → `shairport-sync` (stdout) → `wledplay` → UDP → WLED

No `/dev/snd` required.

## Documentation

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — processes and module map
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — unfinished requirements and later work
- [`docs/SYNC_MEASUREMENT.md`](docs/SYNC_MEASUREMENT.md) — visual versus audible delay
- [`SECURITY.md`](SECURITY.md) — trusted-LAN control plane
- [`docs/LICENSING.md`](docs/LICENSING.md)

Research docs (upstream analysis):

- [`docs/WLED_AUDIO_SYNC.md`](docs/WLED_AUDIO_SYNC.md)
- [`docs/WLED_AUDIO_ANALYSIS.md`](docs/WLED_AUDIO_ANALYSIS.md)
- [`docs/SHAIRPORT_INTEGRATION.md`](docs/SHAIRPORT_INTEGRATION.md)

Design records, not the operator manual:

- [`docs/design/software-requirements.md`](docs/design/software-requirements.md)
- [`docs/design/auto-join.md`](docs/design/auto-join.md)

## Development

See [`CONTRIBUTING.md`](CONTRIBUTING.md). The toolchain is Rust 1.89 ([`rust-toolchain.toml`](rust-toolchain.toml)). Feeding a PCM file without AirPlay is documented there.

## License

MIT — see [`LICENSE`](LICENSE). Docker image bundles GPL components separately; see [`docker/NOTICE`](docker/NOTICE).
