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

WLEDPlay runs on a Linux host or Linux container using host networking. NQPTP needs UDP 319 and 320, which macOS already uses, so the receiver stack cannot run on macOS. Docker Desktop does not provide the required LAN host networking for AirPlay discovery.

```bash
./scripts/ensure-config.sh
WLEDPLAY_DESTINATION=192.0.2.10 \
WLEDPLAY_AIRPLAY_INTERFACE=eth1 \
WLEDPLAY_UNICAST_INTERFACE=eth2 \
WLEDPLAY_MULTICAST_INTERFACE=eth2 \
docker compose up -d --build
```

`192.0.2.10` is a documentation address. Substitute a WLED controller address. Leave a NIC variable unset when that traffic does not need interface pinning. Compose builds this checkout and tags the local image as `wledplay:local` by default.

Without Docker, on a Linux host:

```bash
./scripts/build-wledplay.sh
./scripts/run-wledplay.sh
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
- `POST /wled/measure` — estimate each controller's delay from its LED count, frame rate, and a short network sample. The table stores those delays on Control Save
- `POST /restart` — orderly process restart
- `GET /logs` — lines from the WLEDPlay, Shairport Sync, NQPTP, or console log. `source` selects the file (`wledplay` by default). `tail`, `offset`, `before`, `q`, and `limit` select the window
- `GET /logs/download` — that same file as a download
- `PATCH /config` with `log_level` — sets the WLEDPlay tracing level (`error`, `warn`, `info`, `debug`, or `trace`) immediately and stores `logging.level`

## Configuration

Copy the examples to gitignored local files and edit. Pulls update the examples only. They never overwrite local destinations, NIC pins, delay, or Shairport buffers.

```bash
./scripts/ensure-config.sh
```

That copies [`config/config-example.yaml`](config/config-example.yaml) to `config/config.yaml` and [`config/shairport-sync-example.conf`](config/shairport-sync-example.conf) to `config/shairport-sync.conf` when those files are missing. Docker Compose mounts `./config` and uses the local Shairport file if present, otherwise the example. Name, NIC pins, and Shairport lead are patched at start from yaml or env. Docker always creates the metadata FIFO at `WLEDPLAY_METADATA_PIPE` (default `/run/wledplay.meta`) and passes it to Shairport and `wledplay`. An empty `metadata.pipe` in yaml does not turn metadata off inside the container.

On a multi-homed host, pin each traffic type to a NIC (`ip -br addr`): HTTP (management UI), AirPlay/mDNS (user VLAN), unicast (WLED device IPs), and multicast (`239.0.0.1`, often a third IoT NIC). Empty means no pin. HTTP then uses `0.0.0.0:8080`. The entrypoint detects an existing host Avahi socket and reuses host Avahi; otherwise it starts D-Bus and Avahi in the container. `WLEDPLAY_AVAHI_MODE=auto|host|container` selects automatic detection or forces a mode. Host mode requires host D-Bus and Avahi sockets, which Compose mounts read-only; see [`SECURITY.md`](SECURITY.md) for the trust implications. Legacy `ENABLE_AVAHI=0|1` remains supported (`0` = host, `1` = container).

| Variable | Purpose |
|----------|---------|
| `WLEDPLAY_AIRPLAY_NAME` | Shairport Sync advertised name |
| `WLEDPLAY_MULTICAST_ADDRESS` | Default `239.0.0.1` |
| `WLEDPLAY_PORT` | Default `11988` |
| `WLEDPLAY_DESTINATION` | Comma-separated unicast WLED IPs (Docker) |
| `WLEDPLAY_HTTP_INTERFACE` | NIC for the HTTP UI (empty = `web.listen` / all interfaces) |
| `WLEDPLAY_AIRPLAY_INTERFACE` | NIC for Shairport. Avahi listens here, and the AirPlay browser uses only this NIC |
| `WLEDPLAY_UNICAST_INTERFACE` | NIC for WLED unicast UDP. When this differs from the AirPlay NIC, Avahi listens on it too, and the WLED browser uses only this NIC |
| `WLEDPLAY_MULTICAST_INTERFACE` | NIC for WLED multicast (`239.0.0.1`) |
| `WLEDPLAY_WEB_LISTEN` | HTTP bind address when HTTP NIC is unset (default `0.0.0.0`) |
| `WLEDPLAY_WEB_PORT` | HTTP port (default `8080`) |
| `WLEDPLAY_METADATA_PIPE` | Shairport metadata FIFO (Docker: `/run/wledplay.meta`) |
| `WLEDPLAY_SHAIRPORT_LATENCY_OFFSET` | Shairport `audio_backend_latency_offset_in_seconds` (e.g. `-1.2`) |
| `WLEDPLAY_MULTICAST_ENABLED` | `false` / `0` disables multicast send (unicast still works) |
| `WLEDPLAY_AVAHI_MODE` | `auto` (default) reuses host Avahi when detected, otherwise starts it in the container; force with `host` or `container` |
| `ENABLE_AVAHI` | Legacy override: `0` uses host Avahi; `1` starts D-Bus + Avahi in the container |
| `WLEDPLAY_IPV4_ONLY` | In container Avahi mode, `1` sets `use-ipv6=no`. Does not change host IPv6 or Shairport's timing-peer list |
| `WLEDPLAY_SHAIRPORT_PATCH` | Build arg. `0` (default) builds stock 5.6.3; `1` applies `docker/shairport-mixed-stack.patch` |
| `WLEDPLAY_NQPTP_PATCH` | Build arg. `0` (default) builds stock 1.2.9; `1` applies `docker/nqptp-timing-peers.patch` |
| `WLEDPLAY_SYNC_OFFSET_MS` | Live send delay in milliseconds (`audio.sync_offset_ms`). Negative values clamp to `0` |
| `WLEDPLAY_LOG_LEVEL` | Log level at startup (`logging.level`). The Logs card can change the running level |
| `WLEDPLAY_LOG_DIR` | Directory of `wledplay.log`, `shairport.log`, `nqptp.log`, and `console.log` (default `/var/log/wledplay`) |

## Docker

Requires **host networking** (mDNS, multicast, AirPlay 2 PTP).

The entrypoint selects Avahi mode inside the container: it reuses host D-Bus/Avahi when the host Avahi socket is mounted, otherwise it starts container D-Bus and Avahi. `WLEDPLAY_AVAHI_MODE=host` or `container` can force either mode. In host mode, the root-run container connects to the host system bus; review the trust implications in [`SECURITY.md`](SECURITY.md).

Stack after mDNS setup: `nqptp` → `shairport-sync` (stdout) → `wledplay` → UDP → WLED

The entrypoint also writes `/var/log/wledplay/`. WLEDPlay and NQPTP logs are their own output, Shairport's log is stderr (stdout stays the PCM FIFO), and `console.log` is the combined container output. Each file keeps its last 2 MB after it grows past 8 MB. The Logs card reads those files. WLEDPlay lines are stamped in the container's local time with a numeric offset such as `-05:00` or `+00:00`. Compose mounts `/etc/localtime` so that clock matches the host, and the image includes `tzdata` so an explicit `TZ` works when the mount is absent. Debug and trace add one analysis line per second while audio is playing (RMS, a peak latched across that second, frequency, FFT bands, UDP sends, audio frames, delay-queue drops, and the timing offset), and another line when one of those counters changes while idle.

Compose grants `SYS_NICE` so Shairport can set realtime scheduling on its audio thread. An `rtprio` limit is not set: the container runtime rejects it and refuses to start. The entrypoint waits for the Auto Join sidecar port, retrying for about five seconds, before starting WLEDPlay. If `/dev/shm/nqptp` is not ready within 5 seconds, startup continues and a later line reports when the clock file appears. A playing session also warns when that file is still missing. Track title and artist are logged once, when a metadata bundle completes. Cover art is replaced when the track changes, and `GET /status` includes `metadata.artwork_rev` so the UI reloads `/artwork`. The same snapshot includes a once-per-second CPU, memory, send lateness, and jitter sample, plus the delay-queue drop count.

No `/dev/snd` required.

### Published container images (GHCR)

GitHub Actions publishes multi-arch images to `ghcr.io/<repo-owner>/wledplay` from `.github/workflows/docker-publish.yml`.

- Release tags like `v0.3.0` publish images (release-only; no publish on plain branch pushes).
- A stable release tag publishes `0.3.0`, `0.3`, `0`, and `latest`.
- Pre-release tags (for example `v0.4.0-rc.1`) publish semver/SHA tags but skip `latest`.
- Published images target `linux/amd64` and `linux/arm64`.

To run from a published image instead of a local build, set `WLEDPLAY_IMAGE` to `ghcr.io/<repository-owner>/wledplay:<tag>` and skip building:

```bash
export WLEDPLAY_IMAGE=ghcr.io/OWNER/wledplay:latest
docker compose pull
docker compose up -d --no-build
```

### Development images

To build a multi-platform development image locally for remote Linux test hosts, first commit and push the source to `dev`, then authenticate to GHCR once:

```bash
git switch dev
git add -p
git commit -m "Describe the change"
git push
docker login ghcr.io
./scripts/build-dev.sh
```

`build-dev.sh` requires a clean worktree whose `HEAD` matches `origin/dev`. It derives the GHCR owner from the `origin` remote; set `WLEDPLAY_DEV_IMAGE=ghcr.io/OWNER/wledplay` to override it. The script publishes both `dev` and `dev-<commit>` tags for `linux/amd64` and `linux/arm64`; use the printed commit-specific tag on test hosts. The app displays Cargo's package version plus `+dev.<commit>`, keeping `Cargo.toml` as the sole release-version source. The authenticated account needs package write access, and the package must be public for anonymous pulls.

On each test host, update the source branch and run the matching image without a local build:

```bash
git switch dev
git pull --ff-only
export WLEDPLAY_IMAGE=ghcr.io/OWNER/wledplay:dev-COMMIT
docker compose pull
docker compose up -d --no-build --pull never
```

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

See [`CONTRIBUTING.md`](CONTRIBUTING.md). The toolchain is Rust 1.100.0 ([`rust-toolchain.toml`](rust-toolchain.toml)). Feeding a PCM file without AirPlay is documented there.

## License

MIT — see [`LICENSE`](LICENSE). Docker image bundles GPL components separately; see [`docker/NOTICE`](docker/NOTICE).
