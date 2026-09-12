# Shairport Sync Integration

WLEDPlay uses upstream **Shairport Sync** and **NQPTP** as replaceable external processes. WLEDPlay reads decoded PCM only; it does not implement AirPlay.

---

## Recommended versions

| Component | Recommendation | Notes |
|-----------|----------------|-------|
| Shairport Sync | Latest stable **4.x** release from [mikebrady/shairport-sync](https://github.com/mikebrady/shairport-sync) | Build with `--with-stdout`, `--with-airplay-2`, `--with-soxr` or `--with-sw-resample` |
| NQPTP | Latest from [mikebrady/nqptp](https://github.com/mikebrady/nqptp) | Required for AirPlay 2 timing |

Pin exact versions in Docker image tags for reproducibility.

---

## AirPlay 1 vs AirPlay 2

| Feature | AirPlay 1 | AirPlay 2 |
|---------|-----------|-----------|
| Timing | Shairport Sync internal | NQPTP PTP clock |
| Default PCM (stdout) | S16_LE, **44100 Hz**, stereo | S32_LE, **48000 Hz**, stereo (transcode configurable) |
| Multi-room | Limited | Native synchronized groups |
| Build flags | Base shairport-sync | `--with-airplay-2` + run **nqptp** |

Phase 3 is AirPlay 1 on macOS. Phase 4 is AirPlay 2 + NQPTP on **Linux**.

### macOS (AirPlay 1 only)

Homebrew `shairport-sync` is **AirPlay 1** (`--with-stdout`, `--with-dns_sd`). Use [`scripts/run-airplay.sh`](../scripts/run-airplay.sh).

macOS **AirPlay Receiver** listens on TCP **5000** — turn it off before the AP1 runner.

**AirPlay 2 cannot run on macOS.** NQPTP needs exclusive UDP 319/320, which the OS already uses ([AIRPLAY2.md](https://github.com/mikebrady/shairport-sync/blob/master/AIRPLAY2.md)).

### Linux (AirPlay 2)

Pinned versions: Shairport Sync **5.5.1** + NQPTP **1.2.8** (`smi10`).

```bash
./scripts/build-airplay2.sh
./scripts/run-airplay2.sh
```

Config: [`config/shairport-sync-airplay2.conf`](../config/shairport-sync-airplay2.conf) (`service_type = "airplay2"`, stdout **48000 / S16_LE / 2**). Docker uses the same pairing with `network_mode: host`.

---

## Headless PCM output (no `/dev/snd`)

Build Shairport Sync with stdout backend:

```bash
./configure --with-stdout --with-airplay-2 --with-avahi --with-ssl=openssl
make
```

Configuration (`/etc/shairport-sync.conf` or `docker/shairport-sync.conf`):

```conf
general = {
  name = "WLEDPlay";
  output_backend = "stdout";
  // AirPlay 2: ensure nqptp is running
};

stdout = {
  output_rate = 44100;
  output_format = "S16_LE";
  output_channels = 2;
  audio_backend_buffer_desired_length = 1.0;
  audio_backend_latency_offset = 0.0;
};
```

Run pipeline:

```bash
nqptp &
shairport-sync -c /etc/shairport-sync.conf | wledplay --input stdin \
  --pcm-rate 44100 --pcm-channels 2 --pcm-format s16le
```

For AirPlay 2, set stdout to fixed **48000 / S16_LE / 2** (or S32 with WLEDPlay format support) to avoid silent format changes mid-stream.

---

## PCM format handed to WLEDPlay

| Parameter | AirPlay 1 default | AirPlay 2 default | WLEDPlay internal |
|-----------|-------------------|-------------------|-------------------|
| Rate | 44100 | 48000 | **22050** (WLED analysis rate) |
| Channels | 2 interleaved | 2 | **1** (mono downmix) |
| Format | S16_LE | S32_LE (configurable) | float analysis |
| Frame order | LRLRLR… | LRLRLR… | mono samples |

WLEDPlay resamples and downmixes before analysis.

---

## Metadata

Shairport Sync can expose D-Bus/metadata (title, artist, album, artwork) when built with metadata support. WLEDPlay Phase 6+ may read metadata via:

- Shairport Sync `-M` / metadata socket
- Parsing companion log output

Not required for audio-reactive operation.

---

## Presentation timing

Shairport Sync tracks playout timing for synchronized AirPlay 2 output. For stdout backend:

- PCM is written when decoded for playout
- `audio_backend_latency_offset` adjusts backend timing
- WLEDPlay should measure visual vs audible sync in multi-room tests before applying `audio.sync_offset_ms`

Distinguish:

```text
audio received → decoded → analyzed → (AirPlay presentation time on speakers)
```

Initial implementation: analyze and transmit immediately; document measured offset.

For AirPlay 1 on macOS we shrink Shairport’s decoded/backend buffers and set
`audio_backend_latency_offset_in_seconds = -1.0` so PCM is written ~1 s before
presentation time. Tune that value (more negative = earlier visuals) if the
matrix still lags the speakers. WLEDPlay also drops stdin backlog older than
~80 ms so a pause does not keep draining seconds of queued audio.

---

## Startup / shutdown

Container entrypoint order:

1. Start **dbus** + **avahi-daemon** (mDNS; pin to `WLEDPLAY_INTERFACE` when set)
2. Start **nqptp** (AirPlay 2)
3. Start **wledplay** (reads pipe)
4. Start **shairport-sync** (stdout → pipe)

On SIGTERM:

1. Stop wledplay (flush UDP, close HTTP)
2. SIGTERM shairport-sync
3. Stop nqptp

Restart policy: if shairport-sync exits, supervisor restarts full stack.

---

## Docker networking

Use **`network_mode: host`** because WLEDPlay requires:

- Bonjour/mDNS (AirPlay advertisement)
- UDP multicast to WLED (`239.0.0.1:11988`)
- AirPlay 2 PTP (NQPTP)

No `- /dev/snd` mount required.

---

## Docker Compose example

See [`docker-compose.yml`](../docker-compose.yml) and [`docker/entrypoint.sh`](../docker/entrypoint.sh).

---

## References

- [Shairport Sync sample config](https://github.com/mikebrady/shairport-sync/blob/master/scripts/shairport-sync.conf)
- [CONFIGURATION FLAGS](https://github.com/mikebrady/shairport-sync/blob/master/CONFIGURATION%20FLAGS.md)
- [`audio_stdout.c`](https://github.com/mikebrady/shairport-sync/blob/master/audio_stdout.c)
- [NQPTP](https://github.com/mikebrady/nqptp)
