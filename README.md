# WLEDPlay

Headless AirPlay-to-WLED audio-reactive bridge. Presents as an AirPlay receiver, analyzes decoded PCM, and transmits WLED Audio Sync V2 packets to one or more WLED 16.x controllers.

## Quick start (PCM test, no AirPlay)

```bash
cargo build --release

# Generate test tone (440 Hz sine, 44100 Hz stereo S16LE, 5 seconds)
python3 scripts/gen_test_pcm.py -o /tmp/test.pcm

# Set WLED Audio Reactive → Sync Mode = Receive, port 11988
cat /tmp/test.pcm | ./target/release/wledplay --config config/config.yaml --input stdin
```

WLEDPlay stops UDP transmission shortly after the PCM stream ends and exits when stdin closes (pipe tests).

## AirPlay (Phase 3)

On macOS, Docker Desktop cannot advertise AirPlay on the LAN (`network_mode: host` is a Linux feature). Run Shairport Sync natively and pipe PCM into WLEDPlay:

```bash
brew install shairport-sync
./scripts/run-airplay.sh
```

Then pick **WLEDPlay** in Control Center / Apple Music. The Homebrew formula is AirPlay 1 with a `stdout` backend — that is enough for a phone or Mac to stream audio.

On macOS, **turn off AirPlay Receiver** first (`System Settings → General → AirDrop & Handoff`). Control Center already listens on TCP 5000, and Shairport Sync 5.5 always uses that port for AirPlay 1.

Use `--no-realtime` whenever Shairport (or any live source) already emits PCM in real time. `cat file.pcm | …` still needs pacing (the default).

## AirPlay 2 (Phase 4)

Official Shairport Sync AirPlay 2 uses **NQPTP** on UDP **319/320**. macOS already owns those ports, so AP2 **cannot run on this Mac**. Stay on `./scripts/run-airplay.sh` here.

On a Linux host (Pi, NUC, etc.):

```bash
./scripts/build-airplay2.sh
./scripts/run-airplay2.sh
```

Or:

```bash
WLEDPLAY_DESTINATION=10.11.1.124 WLEDPLAY_INTERFACE=eth1 docker compose up -d --build
```

On a multi-homed host, set `WLEDPLAY_INTERFACE` to the VLAN NIC (`ip -br addr`). That binds Shairport, Avahi, WLED UDP, and `/status` to that interface. The container starts its own D-Bus + Avahi; if the host already owns UDP 5353, stop host `avahi-daemon` or set `ENABLE_AVAHI=0` and mount `/run/dbus`.

Then group **WLEDPlay** with a HomePod / Apple TV and measure visual vs audible sync (`docs/SYNC_MEASUREMENT.md`). AP2 PCM is pinned to **48000 Hz S16LE**.

## Configuration

See [`config/config.yaml`](config/config.yaml). Environment overrides:

| Variable | Purpose |
|----------|---------|
| `WLEDPLAY_AIRPLAY_NAME` | Shairport Sync advertised name |
| `WLEDPLAY_MULTICAST_ADDRESS` | Default `239.0.0.1` |
| `WLEDPLAY_PORT` | Default `11988` |
| `WLEDPLAY_DESTINATION` | Comma-separated unicast WLED IPs (Docker) |
| `WLEDPLAY_INTERFACE` | Host NIC name (e.g. `eth1`) for AirPlay + Avahi + UDP + HTTP |
| `ENABLE_AVAHI` | `1` (default) starts D-Bus + Avahi in the container; `0` uses host Avahi |
| `WLEDPLAY_SYNC_OFFSET_MS` | Timing compensation (reserved) |
| `WLEDPLAY_LOG_LEVEL` | Log level |

## HTTP status

- `GET /health` — liveness
- `GET /status` — JSON runtime state (default `:8080`)

## Docker (Linux host)

Requires **host networking** (mDNS, multicast, AirPlay 2 PTP). This does not work as a LAN AirPlay receiver on Docker Desktop for Mac.

```bash
WLEDPLAY_DESTINATION=10.11.1.124 WLEDPLAY_INTERFACE=eth1 docker compose up -d --build
```

Stack: `dbus` + `avahi` → `nqptp` → `shairport-sync` (stdout) → `wledplay` → UDP → WLED

No `/dev/snd` required.

## WLED setup

1. Flash official WLED **16.x** (e.g. v16.0.1+)
2. Enable **Audio Reactive** usermod
3. Set **Sync Mode = Receive**
4. Match UDP port (`11988` default)

### VLAN / firewall requirement

WLEDPlay sends **UDP** to your controller. If your Mac/PC and WLED are on **different VLANs** (e.g. Mac on `10.3.x`, WLED on `10.11.x`), HTTP to the WLED web UI may work while **UDP 11988 is blocked**. WLED will show **Audio Source: UDP sound sync - idle** even when WLEDPlay is running.

Verify:

```bash
python3 scripts/verify_wled_sync.py 10.11.1.124
```

**Fix:** run WLEDPlay on a host in the **same VLAN as WLED** (IoT / `10.11.x`), or add a firewall rule allowing **UDP 11988** to the WLED IP. Multicast (`239.0.0.1`) does not cross VLANs; unicast to the device IP from the same VLAN is the reliable path.

On UniFi: create a rule allowing `10.11.1.124:11988/udp` from your WLEDPlay host (or the IoT VLAN).

## Documentation

Research docs (authoritative upstream analysis):

- [`docs/WLED_AUDIO_SYNC.md`](docs/WLED_AUDIO_SYNC.md)
- [`docs/WLED_AUDIO_ANALYSIS.md`](docs/WLED_AUDIO_ANALYSIS.md)
- [`docs/SHAIRPORT_INTEGRATION.md`](docs/SHAIRPORT_INTEGRATION.md)
- [`docs/LICENSING.md`](docs/LICENSING.md)

## Development

```bash
cargo test
cargo run -- --input stdin --pcm-rate 44100 --pcm-channels 2 < /tmp/test.pcm
```

## License

MIT — see [`LICENSE`](LICENSE). Docker image bundles GPL components separately; see [`docker/NOTICE`](docker/NOTICE).
