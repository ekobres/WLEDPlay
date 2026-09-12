# AirPlay 2 synchronization measurement

WLEDPlay transmits analysis as soon as Shairport writes PCM to stdout. In AirPlay 2 mode, Shairport + NQPTP delay that write to the shared presentation time, so the matrix should line up with HomePods / Apple TVs in the same group.

This measurement **requires a Linux (or FreeBSD) host**. Official AP2 + NQPTP cannot run on macOS (UDP 319/320).

## Procedure

1. On Linux: `./scripts/run-airplay2.sh` or `docker compose up -d --build` with host networking.
2. Confirm `shairport-sync -V` contains `AirPlay2` and `nqptp -V` reports the same SMI version (`smi10` for 5.5.1 / 1.2.8).
3. In Control Center, start a group that includes **WLEDPlay** and a known-good speaker.
4. Play a track with sharp transients (metronome, clap, kick).
5. Watch WLED (PS GEQ 2D) vs the speaker.
6. If the matrix is late, increase `audio.sync_offset_ms` in `config/config.yaml` (WLEDPlay delays the UDP send).
7. If the matrix is early, decrease that value toward 0, or set a small **negative** `audio_backend_latency_offset_in_seconds` in the Shairport AP2 config (PCM written earlier).

Record the chosen offset once it looks right.

## Defaults

| Knob | Default | Meaning |
|------|---------|---------|
| `audio.sync_offset_ms` | `0` | Extra delay inside WLEDPlay before each UDP packet |
| `audio_backend_latency_offset_in_seconds` (AP2) | `0.0` | Shairport write vs presentation time |

Do not copy the AirPlay 1 macOS offset (`-1.0`); that was compensating for classic AirPlay buffering, not multi-room PTP.
