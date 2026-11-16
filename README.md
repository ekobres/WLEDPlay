# WLEDPlay

Headless AirPlay 2-to-WLED audio-reactive bridge. Presents as an AirPlay receiver, analyzes decoded PCM, and transmits WLED Audio Sync V2 packets to one or more WLED 16.x controllers.

**AirPlay 1 is deliberately not supported.** Classic AirPlay cannot join a live speaker group with correct presentation-time sync — the primary use case is grouping WLEDPlay with HomePods, Apple TVs, and other AirPlay 2 speakers.

## Quick start (PCM test, no AirPlay)

```bash
cargo build --release

# Generate test tone (440 Hz sine, 44100 Hz stereo S16LE, 5 seconds)
python3 scripts/gen_test_pcm.py -o /tmp/test.pcm

# Set WLED Audio Reactive → Sync Mode = Receive, port 11988
./scripts/ensure-config.sh
cat /tmp/test.pcm | ./target/release/wledplay --config config/config.yaml --input stdin
```

WLEDPlay stops UDP transmission shortly after the PCM stream ends and exits when stdin closes (pipe tests).

## AirPlay 2 (Linux)

WLEDPlay requires **AirPlay 2** with **NQPTP** (UDP ports 319/320). macOS cannot host this stack; use a Linux host (Pi, NUC, LXC, Docker).

On a Linux host:

```bash
./scripts/build-airplay.sh
./scripts/run-airplay.sh
```

Or Docker with host networking:

```bash
./scripts/ensure-config.sh
WLEDPLAY_DESTINATION=192.0.2.10 \
WLEDPLAY_AIRPLAY_INTERFACE=eth1 \
WLEDPLAY_UNICAST_INTERFACE=eth2 \
WLEDPLAY_MULTICAST_INTERFACE=eth2 \
docker compose up -d --build
```

`192.0.2.10` is a documentation address. Substitute your WLED controller. Compose builds the image on this machine and tags it `ghcr.io/wledplay/wledplay`. That name is not a published registry image; do not `docker pull` it.

On a multi-homed host, pin each traffic type to a NIC (`ip -br addr`): HTTP (management UI), AirPlay/mDNS (user VLAN), unicast (WLED device IPs), and multicast (`239.0.0.1`, often a third IoT NIC). Empty means no pin — HTTP then uses `0.0.0.0:8080`. The container starts its own D-Bus + Avahi; if the host already owns UDP 5353, stop host `avahi-daemon` or set `ENABLE_AVAHI=0` and mount `/run/dbus`.

Group **WLEDPlay** with a HomePod / Apple TV and measure visual vs audible sync ([`docs/SYNC_MEASUREMENT.md`](docs/SYNC_MEASUREMENT.md)). AirPlay PCM is pinned to **48000 Hz S16LE**.

Docker Desktop for Mac cannot advertise AirPlay on the LAN (`network_mode: host` is a Linux feature).

Use `--no-realtime` whenever Shairport (or any live source) already emits PCM in real time. `cat file.pcm | …` still needs pacing (the default).

## Configuration

Copy the examples to gitignored local files and edit. Pulls update the examples only; they never overwrite local destinations, NIC pins, delay, or Shairport buffers.

```bash
./scripts/ensure-config.sh
```

That copies [`config/config-example.yaml`](config/config-example.yaml) → `config/config.yaml` and [`config/shairport-sync-example.conf`](config/shairport-sync-example.conf) → `config/shairport-sync.conf` when those files are missing. Docker Compose mounts `./config` and uses the local Shairport file if present, otherwise the example. Name, NIC pins, and Shairport lead are patched at start from yaml or env. Docker always creates the metadata FIFO at `WLEDPLAY_METADATA_PIPE` (default `/run/wledplay.meta`) and passes it to Shairport and `wledplay`. An empty `metadata.pipe` in yaml does not turn metadata off inside the container.

Environment overrides:

| Variable | Purpose |
|----------|---------|
| `WLEDPLAY_AIRPLAY_NAME` | Shairport Sync advertised name |
| `WLEDPLAY_MULTICAST_ADDRESS` | Default `239.0.0.1` |
| `WLEDPLAY_PORT` | Default `11988` |
| `WLEDPLAY_DESTINATION` | Comma-separated unicast WLED IPs (Docker) |
| `WLEDPLAY_HTTP_INTERFACE` | NIC for the HTTP UI (empty = `web.listen` / all interfaces) |
| `WLEDPLAY_AIRPLAY_INTERFACE` | NIC for Shairport + Avahi (mDNS / AirPlay) |
| `WLEDPLAY_UNICAST_INTERFACE` | NIC for WLED unicast UDP |
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

## HTTP status

The operator UI listens on `0.0.0.0:8080` by default and does not authenticate. `PATCH /config` and `POST /restart` are open to anyone who can reach that port. Bind `network.http` or `WLEDPLAY_HTTP_INTERFACE` to a management NIC, or firewall TCP 8080. See [`SECURITY.md`](SECURITY.md).

- `GET /` — operator UI
- `GET /health` — liveness
- `GET /status` — JSON runtime state (default `:8080`)
- `GET /levels` — slim live meters (RMS, peak, frequency, 16 bands) for the UI poll
- `GET /artwork` — current cover art (404 if none)
- `GET /config` / `PATCH /config` — live settings
- `GET /interfaces` — usable IPv4 NICs
- `GET /airplay/devices` — AirPlay receivers Auto Join can see
- `POST /restart` — orderly process restart

## Docker (Linux host)

Requires **host networking** (mDNS, multicast, AirPlay 2 PTP). This does not work as a LAN AirPlay receiver on Docker Desktop for Mac.

```bash
./scripts/ensure-config.sh
WLEDPLAY_DESTINATION=192.0.2.10 \
WLEDPLAY_AIRPLAY_INTERFACE=eth1 \
WLEDPLAY_UNICAST_INTERFACE=eth2 \
WLEDPLAY_MULTICAST_INTERFACE=eth2 \
docker compose up -d --build
```

Stack: `dbus` + `avahi` → `nqptp` → `shairport-sync` (stdout) → `wledplay` → UDP → WLED

No `/dev/snd` required.

## WLED setup

1. Flash official WLED **16.x** (e.g. v16.0.1+)
2. Enable **Audio Reactive** usermod
3. Set **Sync Mode = Receive**
4. Match UDP port (`11988` default)

### VLAN / firewall requirement

WLEDPlay sends **UDP** to your controller. If the computer running WLEDPlay and the WLED controller are on different VLANs, HTTP to the WLED web UI may work while **UDP 11988 is blocked**. WLED will show **Audio Source: UDP sound sync - idle** even when WLEDPlay is running.

Verify:

```bash
python3 scripts/verify_wled_sync.py 192.0.2.10
```

**Fix:** run WLEDPlay on a host in the **same VLAN as WLED**, or add a firewall rule allowing **UDP 11988** to the WLED IP. Multicast (`239.0.0.1`) does not cross VLANs; unicast to the device IP from the same VLAN is the reliable path. On a multi-homed host set `network.unicast` / `network.multicast` (or the matching env vars) to the WLED VLAN NIC so `239.0.0.1` is not sent out the management default route. IGMP snooping may keep that group off the router trunk if no receiver on that path has joined.

Example: allow `192.0.2.10:11988/udp` from the WLEDPlay host. Substitute your controller address.

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

See [`CONTRIBUTING.md`](CONTRIBUTING.md). The toolchain is Rust 1.89 ([`rust-toolchain.toml`](rust-toolchain.toml)).

```bash
cargo fmt           # format (uses rustfmt.toml)
cargo fmt --check   # CI: fail if formatting differs
cargo clippy        # lint
cargo test          # unit tests (src/**) + integration tests (tests/**)
cargo test --lib    # library unit tests only
cargo test --test analysis_tests

# Line coverage (Homebrew rustc: point llvm-cov at LLVM)
export LLVM_COV="$(brew --prefix llvm)/bin/llvm-cov"
export LLVM_PROFDATA="$(brew --prefix llvm)/bin/llvm-profdata"
cargo llvm-cov --html --output-dir /tmp/wledplay-html
cargo llvm-cov report --fail-under-lines 100
```

Unit tests live next to the code they cover (`#[cfg(test)] mod tests` in each module). Integration tests in `tests/` exercise YAML loading, protocol roundtrips, and end-to-end analyzer behavior.

```bash
cargo run -- --input stdin --pcm-rate 44100 --pcm-channels 2 < /tmp/test.pcm
```

## License

MIT — see [`LICENSE`](LICENSE). Docker image bundles GPL components separately; see [`docker/NOTICE`](docker/NOTICE).
