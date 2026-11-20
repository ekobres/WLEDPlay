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
| **pyatv** 0.16.1 | **MIT** | [LICENSE.md](https://github.com/postlund/pyatv/blob/master/LICENSE.md) | Auto Join sidecar; not linked into the Rust binary |
| **srptools** 1.0.1 | **BSD-3-Clause** | [srptools](https://github.com/idlesign/srptools) | Dependency of pyatv, installed in the image |
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
2. **Docker image:** Aggregates independent programs (like a Linux distro). The image runs Shairport Sync and NQPTP as separate processes.
3. **No static linking** of Shairport Sync into WLEDPlay.

A default image build ships the unmodified Shairport Sync tag **5.6.3** and NQPTP tag **1.2.9**. Those tags are cloned by name, not by commit SHA. Set `WLEDPLAY_SHAIRPORT_PATCH=1` or `WLEDPLAY_NQPTP_PATCH=1` to opt into the matching local patch.

Corresponding source for a patched binary is the upstream tag plus the patch file in this repository. [`docker/NOTICE`](../docker/NOTICE) states that offer. WLEDPlay's own code stays MIT.

Docker image licensing is not WLEDPlay binary licensing. This repository does not publish a container image. Someone who builds the Dockerfile and gives the image to others still has to pass along this source offer.

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

Source release (this repository):

- [x] [`LICENSE`](../LICENSE) is MIT for WLEDPlay
- [x] [`docker/NOTICE`](../docker/NOTICE) names Shairport Sync 5.6.3, NQPTP 1.2.9, both patch files, and the pip packages
- [x] GPL corresponding source is the named upstream tag plus the patch in this repo
- [x] No Apple proprietary assets or DRM circumvention tools

The Dockerfile copies `LICENSE` and `NOTICE` into the image. Publishing that image to a registry is a separate step and is not part of the source release. A published image tag must point at the git commit that contains the patches it was built with.

---

## Patent / DRM

WLEDPlay does not circumvent DRM. It receives legitimately streamed AirPlay audio on the LAN using licensed receiver software (Shairport Sync).
