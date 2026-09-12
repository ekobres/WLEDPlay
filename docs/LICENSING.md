# Licensing

WLEDPlay is designed to keep **WLEDPlay-specific code** under a permissive license while **packaging** separately licensed binaries in Docker.

---

## Component licenses

| Component | License | Link | WLEDPlay relationship |
|-----------|---------|------|----------------------|
| **WLEDPlay** (this repo) | **MIT** | [`LICENSE`](../LICENSE) | Original Rust analyzer + tooling |
| **WLED** | **EUPL-1.2** | [LICENSE](https://github.com/wled/WLED/blob/v16.0.1/LICENSE) | Reference only; no code copied |
| **Shairport Sync** | **GPL-2.0+** (per-file; see upstream) | [shairport-sync](https://github.com/mikebrady/shairport-sync) | Separate process in Docker |
| **NQPTP** | **GPL-2.0** | [nqptp LICENSE](https://github.com/mikebrady/nqptp/blob/master/LICENSE) | Separate process in Docker |
| **Feed_My_WLED** | **GPL-3.0** | [feed_my_wled](https://github.com/chrisgott/feed_my_wled) | Reference only; do not copy |
| **ESP Air Sync** | Check upstream (Codeberg) | [esp-airsync](https://codeberg.org/Edu_Coder/esp-airsync) | Reference only; do not copy |
| **rustfft** | **MIT OR Apache-2.0** | [rustfft](https://github.com/HEnquist/rustfft) | Rust dependency |
| **realfft** | **MIT OR Apache-2.0** | companion to rustfft | Rust dependency |

---

## WLED (EUPL-1.2)

WLED firmware is EUPL-1.2. WLEDPlay:

- Reads WLED source to document protocol and analysis behavior
- Does **not** embed WLED code in the WLEDPlay binary
- Sends compatible UDP packets (protocol facts are not copyrightable; implementation is independent)

If WLEDPlay code were substantially derived from WLED, EUPL copyleft could apply. Current design: clean-room Rust implementation from specification.

---

## GPL components (Shairport Sync, NQPTP)

GPL applies to **distribution of combined works**. Mitigation:

1. **Process isolation:** WLEDPlay binary does not link GPL code.
2. **Docker image:** Aggregates independent programs (like a Linux distro). Provide:
   - Source offer for GPL components
   - `NOTICE` file listing bundled licenses
3. **No static linking** of Shairport Sync into WLEDPlay.

Docker image licensing ≠ WLEDPlay binary licensing.

---

## Reference implementations

| Project | Reuse policy |
|---------|--------------|
| Feed_My_WLED (GPL-3.0) | Study packet format; do not copy source |
| ESP Air Sync | Verify license before any reuse; reference FFT mapping ideas only |
| WLED `audio_reactive.cpp` (EUPL) | Document behavior; independent implementation |

---

## WLEDPlay license choice

**MIT License** for WLEDPlay-specific code:

- Permits proprietary downstream use of analyzer alone (without GPL Docker stack)
- Compatible with common Rust crates (MIT/Apache-2.0)
- Does not infect Docker GPL components when distributed as aggregate

---

## Distribution checklist

When publishing Docker images:

- [ ] Include [`LICENSE`](../LICENSE) (MIT) for WLEDPlay
- [ ] Include `docker/NOTICE` with Shairport Sync + NQPTP GPL notices
- [ ] Document source URLs for GPL binaries
- [ ] Do not commit Apple proprietary assets or DRM circumvention tools

---

## Patent / DRM

WLEDPlay does not circumvent DRM. It receives legitimately streamed AirPlay audio on the LAN using licensed receiver software (Shairport Sync).
