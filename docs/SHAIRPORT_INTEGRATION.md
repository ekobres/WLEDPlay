# Shairport Sync Integration

WLEDPlay uses upstream **Shairport Sync** (AirPlay 2) and **NQPTP** as replaceable external processes. WLEDPlay reads decoded PCM only; it does not implement AirPlay.

**AirPlay 1 is deliberately not supported.** Classic AirPlay cannot join a live speaker group with correct presentation-time sync. WLEDPlay targets AirPlay 2 multi-room grouping with HomePods, Apple TVs, and other synchronized receivers.

---

## Recommended versions

| Component | Recommendation | Notes |
|-----------|----------------|-------|
| Shairport Sync | **5.6.3** (git tag in the Dockerfile) | Build with `--with-stdout`, `--with-airplay-2`, `--with-soxr`, `--with-metadata` |
| NQPTP | **1.2.9** (git tag in the Dockerfile) | Required for AirPlay 2 timing (`smi11`). Must match the Shairport Sync shared-memory version. |

The Dockerfile clones those tags with `git clone --depth 1 --branch`. That pins the tag name, not a commit SHA. A moved tag would change the build. Commit pins are a follow-up for the next version bump.

The image builds unmodified upstream tags by default. Both local patches remain available as opt-in build args; set a switch to `1` to apply that patch.

| Arg | `0` (default) | `1` |
|-----|---------------|-----|
| `WLEDPLAY_SHAIRPORT_PATCH` | Stock 5.6.3 | [docker/shairport-mixed-stack.patch](../docker/shairport-mixed-stack.patch): timing `T` list at SETUP (no `B`, no play lock), timing peers limited to the AirPlay interface, link-local IPv6 omitted, stdout plays before a PTP lock, `ssnc/lead` metadata |
| `WLEDPLAY_NQPTP_PATCH` | Stock 1.2.9, which keeps the first address and drops the rest | [docker/nqptp-timing-peers.patch](../docker/nqptp-timing-peers.patch): NQPTP watches every address in a `T` command |

`scripts/build-airplay-deps.sh` reads the same two variables. A patched Shairport describes itself as `5.6.3-dirty-...`. A stock build of the tag does not say `dirty`. That is the check that the switch took effect.

Build the patched image by opting in:

```bash
WLEDPLAY_SHAIRPORT_PATCH=1 WLEDPLAY_NQPTP_PATCH=1 docker compose build
```

Auto Join does not depend on either patch. It still succeeds only when PCM arrives. Stock Shairport holds stdout until a PTP lock exists, so a group add that delivers audio before the clock locks produces no samples. The NQPTP patch does nothing useful unless the Shairport patch is also on, because stock Shairport sends a single address. Shairport patched with NQPTP stock sends several addresses and NQPTP keeps only the first.

---

## Platform requirements

| Platform | Supported |
|----------|-----------|
| Linux (Docker, bare metal, LXC, Pi) | Yes — Shairport Sync + NQPTP |
| macOS | No — NQPTP needs exclusive UDP 319/320, which the OS already uses ([AIRPLAY2.md](https://github.com/mikebrady/shairport-sync/blob/master/AIRPLAY2.md)) |

The container builds Shairport Sync and NQPTP on Debian 13 Trixie slim and runs them in a matching Trixie runtime image.

Native Linux:

```bash
./scripts/build-wledplay.sh
./scripts/run-wledplay.sh
```

Config: copy [`config/shairport-sync-example.conf`](../config/shairport-sync-example.conf) to gitignored `config/shairport-sync.conf` (`./scripts/ensure-config.sh`). Native Linux and Docker share that file (`service_type = "airplay2"`, stdout **48000 / S16_LE / 2**). Docker Compose mounts `./config` and falls back to the example if the local file is missing.

---

## Headless PCM output (no `/dev/snd`)

Build Shairport Sync with stdout backend:

```bash
./configure --with-stdout --with-airplay-2 --with-avahi --with-ssl=openssl --with-soxr --with-metadata
make
```

Configuration (`config/shairport-sync.conf`, or `/etc/shairport-sync.conf` in the image):

```conf
general = {
  name = "WLEDPlay";
  output_backend = "stdout";
  service_type = "airplay2";
};

stdout = {
  output_rate = 48000;
  output_format = "S16_LE";
  output_channels = 2;
};
```

Run pipeline:

```bash
nqptp &
shairport-sync -c /etc/shairport-sync.conf --service-type=airplay2 -o stdout | \
  wledplay --input stdin --no-realtime \
    --pcm-rate 48000 --pcm-channels 2 --pcm-format s16le
```

Pin stdout to fixed **48000 / S16_LE / 2** to avoid silent format changes mid-stream.

---

## PCM format handed to WLEDPlay

| Parameter | AirPlay 2 (stdout) | WLEDPlay internal |
|-----------|-------------------|-------------------|
| Rate | 48000 | **22050** (WLED analysis rate) |
| Channels | 2 interleaved | **1** (mono downmix) |
| Format | S16_LE (configured) | float analysis |
| Frame order | LRLRLR… | mono samples |

WLEDPlay resamples and downmixes before analysis. The `--pcm-rate 44100` default applies only to file/stdin test input, not the AirPlay path.

---

## Metadata

Shairport Sync exposes title, artist, album, and cover art on a FIFO when built with `--with-metadata`. WLEDPlay reads that pipe (`--metadata-pipe` / `WLEDPLAY_METADATA_PIPE`) and serves it on `/status` and `/artwork`.

Shairport's explicit `ssnc/paus` and `ssnc/pres` events also update `receiver_paused` on `/status` and `paused` on `/levels`. When a pause is observed during the current Auto Join attempt, WLEDPlay keeps waiting for PCM without timing out until playback resumes; the join is not confirmed just because the group request succeeded. The events depend on the receiver session and sender: missing PCM alone is not treated as a confirmed pause.

Metadata is not required for audio-reactive operation.

---

## How iOS / macOS choose the speaker icon

There is **no `shairport-sync.conf` setting** that controls the Control Center icon. The advertised name (`general.name` / `WLEDPLAY_AIRPLAY_NAME`) is only the label.

Apple picks the glyph from the AirPlay **model string**, which Shairport Sync hard-codes in `shairport.c`:

```c
config.model = strdup("ShairportSync");
// config.model = strdup("AudioAccessory5,1");
```

That value is published in three places (see `bonjour_strings.c` and `rtsp.c`):

| Channel | Key | Role |
|---|---|---|
| `_raop._tcp` TXT | `am=` | Classic / RAOP model |
| `_airplay._tcp` TXT | `model=` | AirPlay 2 model |
| RTSP `/info` plist | `model` | Confirmed after connect |

Related keys that do **not** choose the icon:

| Setting | What it actually does |
|---|---|
| `general.name` | Display name only |
| `service_type` (`airplay2` / `classic`) | Protocol / picker grouping |
| `airplay_device_id` / `airplay_device_id_offset` | Stable device identity |
| `features` / `flags` / `srcvers` / `osvers` | Capability bits (hard-coded) |
| `manufacturer` in `/info` | Always `"Shairport Sync"` |

iOS Control Center uses at least three speaker glyphs, all visible in a mixed picker:

| Advertisement | Typical icon |
|---|---|
| Apple model IDs (`AudioAccessory*`, `AppleTV*`, iPhone) | Product silhouette |
| Licensed AirPlay 2 hardware (`model=One` + `manufacturer=Sonos`, Denon AVR, …) | Stylized bookshelf speaker |
| Unknown / software receiver (`model=ShairportSync`, no `manufacturer` TXT) | Speaker cone with sound waves |

WLEDPlay is the third class. Sonos and Denon are the second. They are not the same icon. macOS Control Center often draws both class 2 and class 3 as a bookshelf, which is why they can look similar on a Mac and different on iPhone.

A captured Sonos One TXT record includes `model=One`, `manufacturer=Sonos`, and `features=0x445F8A00,0x1C340`. Shairport Sync advertises `model=ShairportSync` and does **not** put `manufacturer=` in the `_airplay._tcp` TXT. Apple appears to pick the bookshelf only for recognized third-party hardware (model/manufacturer allowlist), not for every AirPlay 2 endpoint.

To look like a HomePod you would rebuild Shairport Sync and set `config.model` to an Apple ID (the commented `AudioAccessory5,1` line). Testers report the HomePod glyph can appear and then snap back on connect. Impersonating Apple or Sonos hardware is unsupported and can break grouping. There is no supported way to become a “licensed speaker” from config.

---

## Presentation timing

Shairport Sync tracks playout timing for synchronized AirPlay 2 output via NQPTP. For stdout backend:

- PCM is written when decoded for playout
- `audio_backend_latency_offset_in_seconds` adjusts backend timing (negative = earlier PCM)
- WLEDPlay exposes **Shairport lead** (restart) and **send delay** (live) as separate yaml/UI controls

Distinguish:

```text
audio received → decoded → analyzed → (AirPlay 2 presentation time on grouped speakers)
```

Default Shairport lead in Docker is `-1.2` seconds with send delay `1200` ms (on time). Tune Shairport lead in ~0.1 s steps after grouping with a reference speaker ([`SYNC_MEASUREMENT.md`](SYNC_MEASUREMENT.md)); trim send delay live in tens of ms. WLEDPlay drops stdin backlog older than ~80 ms so a pause does not keep draining seconds of queued audio.

---

## Startup / shutdown

Container entrypoint order:

1. Start **dbus** + **avahi-daemon** (mDNS; pin to `WLEDPLAY_AIRPLAY_INTERFACE` when set, and also `WLEDPLAY_UNICAST_INTERFACE` when that NIC is different). The AirPlay browser uses the AirPlay NIC. The WLED browser uses the unicast NIC.
2. Start **nqptp** (AirPlay 2)
3. Start the **Auto Join sidecar**
4. Start **wledplay** (reads the PCM pipe)
5. Start **shairport-sync** (stdout → pipe)

WLEDPlay output, NQPTP output, and Shairport stderr are also copied to `/var/log/wledplay`. Shairport stdout stays the PCM FIFO. `console.log` is the combined container output. Each file keeps its last 2 MB after it passes 8 MB.

On SIGTERM the entrypoint stops wledplay, the sidecar, shairport-sync, nqptp, Avahi, and dbus.

If `wledplay` exits, the entrypoint shuts the stack down and the container exits 0. `POST /restart` uses that path. If shairport-sync, nqptp, or the sidecar exits first, the entrypoint shuts the stack down and the container exits 1. Compose `restart: unless-stopped` starts the whole stack again either way.

---

## Docker networking

Use **`network_mode: host`** because WLEDPlay requires:

- Bonjour/mDNS (AirPlay advertisement) on IPv4 and IPv6. With the default Shairport patch, timing peers are the AirPlay interface's global addresses, both families. Link-local addresses and other NICs are left out, and NQPTP watches every address the client offers. A stock build advertises every up, non-loopback address on every NIC.
- UDP multicast to WLED (`239.0.0.1:11988`)
- AirPlay 2 PTP (NQPTP). The entrypoint waits up to 5 seconds for `/dev/shm/nqptp`, then keeps watching for it. Compose grants `SYS_NICE` so Shairport can set realtime scheduling. An `rtprio` limit is not set, because the runtime rejects it and will not start the container.

No `- /dev/snd` mount required.

Per-traffic NIC selection (HTTP, AirPlay/mDNS, unicast, multicast) is configured via `network.*` in YAML or the matching `WLEDPLAY_*_INTERFACE` env vars.

`WLEDPLAY_IPV4_ONLY=1` rewrites the container Avahi config to `use-ipv6=no`. mDNS is then IPv4 only. The container uses the host network namespace, so this does not disable IPv6 on the host and does not change the timing-peer list Shairport builds from its own interfaces. Other IPv4 NICs are still advertised. It is a test lever, not a substitute for the interface filter in the Shairport patch.

---

## Docker Compose example

See [`docker-compose.yml`](../docker-compose.yml) and [`docker/entrypoint.sh`](../docker/entrypoint.sh).

---

## Auto Join spike (pyatv)

WLEDPlay stays a receiver. Auto Join asks the **group leader** to add that receiver. The mechanism is pyatv 0.16 MRP `add_output_devices`, not a second audio path and not an `airport.key`.

Manual probe (Linux, same LAN as the speakers):

```bash
pip install 'pyatv==0.16.1' 'srptools==1.0.1'
python3 scripts/auto-join-spike.py scan --storage config/pyatv/pyatv.json
python3 scripts/auto-join-spike.py watch --storage config/pyatv/pyatv.json \
  --target-id <speaker-id> --receiver-id <wledplay-output-id>
```

Locked contract for the sidecar (`docker/autojoin_sidecar.py`, `127.0.0.1:8091`):

| Question | What the spike uses |
|---|---|
| Stable id | pyatv AirPlay **output device id** when present, otherwise the scan `identifier`. Never the display name. |
| Playing | `playing` → device state `Playing` or `Loading` on the selected target, or on the AirPlay group leader that currently has that speaker in its group. |
| Add output | `audio.add_output_devices` is sent to that group leader, using WLEDPlay’s AirPlay `deviceid` (MAC). pyatv exposes group edits on the audio interface. The leader’s output list must include WLEDPlay before the add counts as sent. |
| Success | Shairport PCM session starts. An MRP `ok` is not success. |
| Pairing | pyatv `FileStorage` file `config/pyatv/pyatv.json` (gitignored). Run the wizard from the compose directory so that relative path exists: `mkdir -p config/pyatv && atvremote --storage-filename config/pyatv/pyatv.json wizard`. Pin `srptools==1.0.1`; srptools 2.0 rejects pyatv's SRP call. A directory path is not a settings file. Never in the browser. |
| iPhone as sender | Best-effort. If the phone owns the session, add-output usually fails. Surface `Join failed`. Direct AirPlay to WLEDPlay keeps working. |
| HomePod / Apple TV as leader | The case this is built for: music started on that device, then WLEDPlay is added as an output. |

`WLEDPLAY_AUTOJOIN_URL` (Docker default `http://127.0.0.1:8091`) points the Rust policy engine at the sidecar. Empty URL leaves Auto Join configured but discovery inactive.

---

## References

- [Shairport Sync sample config](https://github.com/mikebrady/shairport-sync/blob/master/scripts/shairport-sync.conf)
- [CONFIGURATION FLAGS](https://github.com/mikebrady/shairport-sync/blob/master/CONFIGURATION%20FLAGS.md)
- [`audio_stdout.c`](https://github.com/mikebrady/shairport-sync/blob/master/audio_stdout.c)
- [NQPTP](https://github.com/mikebrady/nqptp)
- [pyatv output devices](https://pyatv.dev/documentation/atvremote/)
