# AirPlay 2 synchronization measurement

WLEDPlay transmits analysis as soon as Shairport writes PCM to stdout. In AirPlay 2 mode, Shairport + NQPTP delay that write to the shared presentation time, so the matrix should line up with HomePods / Apple TVs in the same group.

This measurement **requires a Linux (or FreeBSD) host**. Official AP2 + NQPTP cannot run on macOS (UDP 319/320).

## Procedure

1. On Linux: `./scripts/run-airplay.sh` or `docker compose up -d --build` with host networking.
2. Confirm `shairport-sync -V` contains `AirPlay2` and `nqptp -V` reports the same SMI version (`smi10` for 5.5.1 / 1.2.8).
3. In Control Center, start a group that includes **WLEDPlay** and a known-good speaker.
4. Play a track with sharp transients (metronome, clap, kick).
5. Watch WLED (PS GEQ 2D) vs the speaker.
6. If the matrix is **late** (behind the speakers), make `audio_backend_latency_offset_in_seconds` more negative (or set `WLEDPLAY_SHAIRPORT_LATENCY_OFFSET`). That writes PCM earlier.
7. If the matrix is **early**, move that offset back toward `0`. Do not raise `audio.sync_offset_ms` to fix a late matrix — that value only adds more delay.

Record the chosen offset once it looks right.

## Defaults

| Knob | Default | Meaning |
|------|---------|---------|
| `audio.sync_offset_ms` | `0` | Extra delay inside WLEDPlay before each UDP packet (late-only) |
| `audio_backend_latency_offset_in_seconds` (AP2) | `-1.2` | Shairport write vs presentation time (more negative = earlier visuals) |
| `WLEDPLAY_SHAIRPORT_LATENCY_OFFSET` | unset | Docker override for the Shairport offset, seconds |

AirPlay 2 stdout often lands ~1–1.5 s late versus grouped speakers on first connect. Start at `-1.2` and trim by ~0.1 s.
