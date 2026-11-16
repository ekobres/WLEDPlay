# AirPlay 2 synchronization measurement

WLEDPlay transmits analysis as soon as Shairport writes PCM to stdout, then optionally holds each UDP packet for `audio.sync_offset_ms` before send. In AirPlay 2 mode, Shairport + NQPTP target the shared presentation time; the Shairport lead sets how early stdout arrives.

This measurement **requires a Linux (or FreeBSD) host**. Official AP2 + NQPTP cannot run on macOS (UDP 319/320).

## Two knobs

| Setting | yaml | UI | Apply |
|---------|------|-----|-------|
| **Shairport lead** | `audio.shairport_latency_offset_s` (e.g. `-1.2`) | Shairport lead (s) | Restart |
| **Send delay** | `audio.sync_offset_ms` (unsigned ms) | Send delay (ms) | Live |

Send delay semantics (for lead `-1.2` → on-time = `1200` ms):

- `0` — AFAP after decode (up to 1.2 s early vs presentation)
- `1200` — on time (matches presentation)
- `< 1200` — compensate downstream latency (send early)
- `> 1200` — hold past presentation (LEDs late vs heard audio)

## Procedure

1. On Linux: `./scripts/run-airplay.sh` or `docker compose up -d --build` with host networking.
2. Confirm `shairport-sync -V` contains `AirPlay2` and `nqptp -V` reports the same SMI version (`smi11` for 5.6.3 / 1.2.9).
3. In Control Center, start a group that includes **WLEDPlay** and a known-good speaker.
4. Play a track with sharp transients (metronome, clap, kick).
5. Watch WLED (PS GEQ 2D) vs the speaker.
6. If the matrix is **late** (behind the speakers), make `shairport_latency_offset_s` more negative (restart). That writes PCM earlier and increases the hold margin.
7. If the matrix is **early**, move Shairport lead back toward `0` (restart), or raise `sync_offset_ms` in small steps (live, no restart).
8. Do **not** use large `sync_offset_ms` values to fix a matrix that is still late at send delay `0` — that means you need more Shairport lead, not more hold.

Record both values once alignment looks right.

## Shipped example

These values are in [`config/config-example.yaml`](../config/config-example.yaml), which the image copies to `/etc/wledplay/config.yaml`. If the audio fields are absent, the Rust fallback is `sync_offset_ms: 0` and `shairport_latency_offset_s: 0.0`.

| Knob | Shipped example | Meaning |
|------|-----------------|---------|
| `audio.sync_offset_ms` | `1200` | On-time send delay for the shipped `-1.2` s Shairport lead |
| `audio.shairport_latency_offset_s` | `-1.2` | Shairport stdout ~1.2 s early vs presentation (restart) |
| `WLEDPLAY_SHAIRPORT_LATENCY_OFFSET` | unset | Docker override for the Shairport offset, seconds |
| `WLEDPLAY_SYNC_OFFSET_MS` | unset | Live send delay override (negative env values clamp to `0`) |

AirPlay 2 stdout often lands ~1–1.5 s late versus grouped speakers on first connect if Shairport lead is `0`. Start Shairport lead at `-1.2` and trim by ~0.1 s; fine-tune with send delay in tens of ms while streaming.
