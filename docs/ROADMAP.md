# Roadmap

Work that is still open. The design records remain the requirements. This file is the list of what to do next. An item that would change a requirement in [`docs/design/software-requirements.md`](design/software-requirements.md) or [`docs/design/auto-join.md`](design/auto-join.md) needs agreement before any code or doc edit.

Version 0.3.0 already covers the v1 audio path, the operator UI, Docker, and Auto Join. What follows is unfinished work from those records, then items added after them.

## From the software requirements

### Acceptance still open

- **Named analysis tests** ([software requirements §28](design/software-requirements.md)). The suite covers a 440 Hz sine, silence, stereo downmix, and packet roundtrip. It does not yet include the named 100 Hz, 1 kHz, 10 kHz, white-noise, and impulse cases, or the level cases for clipping, a gradual rise, and a transient peak.
- **WLED behavioral comparison** (§29). `scripts/verify_wled_sync.py` checks that UDP arrives. There is no repeatable comparison of WLEDPlay's bands against the same audio analyzed inside WLED.
- **24-hour soak** (§38, item 15). No recorded unattended run through playback and disconnect cycles.

### Deployment still open

- **Native Linux** (§16.1). [`docker/wledplay.service`](../docker/wledplay.service) starts only the analyzer on stdin. Shairport Sync, NQPTP, and Avahi are documented as a Docker stack. A native install of the full receiver is not an operator path, and the unit file is not in the README.
- **Published image** (§17). The requirements show `ghcr.io/<project>/wledplay:latest` as a normal deployment. Compose builds that tag locally. It is not a registry image.

### Deferred features (§39)

These are named and explicitly postponed:

- Album-art palette generation from cover art.
- Automatic WLED discovery over mDNS.
- WLED REST integration: enable Audio Receive, choose presets and palettes, restore state when playback stops.
- Home Assistant: playback state, track, artist, audio level, and status.
- MQTT for stream and metadata.
- Prometheus metrics. The status JSON already carries the §36 counters.
- Optional audio passthrough, without giving up the headless analyzer.

### Optional, not a gap

§32 allows a shared secret or a tighter listen address later. Binding `network.http` is already supported. Authentication is not part of this list until someone decides to add it. The control plane stays a trusted LAN.

## From the Auto Join specification

[Auto Join §3](design/auto-join.md) parks these and says they may be considered later. Several of them would change [§6](design/auto-join.md), which says configuration must never interrupt an active session. They stay parked until that requirement changes:

- More than one Auto Join target.
- Leave a session because another target started playing.
- Join a different group while WLEDPlay is already receiving audio.
- Leave a group because Auto Join was turned off, or because the selected target changed.
- Choose among several groups that are playing at once.

## Added after the requirements

### Retire the Shairport and NQPTP patches

Ship upstream Shairport Sync and NQPTP with no local patches. Remove the patch files, the `WLEDPLAY_SHAIRPORT_PATCH` and `WLEDPLAY_NQPTP_PATCH` build args, and the Dockerfile branches that apply them. Update `docker/NOTICE`, `docs/LICENSING.md`, `docs/SHAIRPORT_INTEGRATION.md`, `docs/ARCHITECTURE.md`, and the README so they stop describing patches that are no longer built.

This matches the requirement to use an upstream Shairport Sync release. Do it once stock AirPlay 2 grouping is the path we support. The patches default on today.

### Debian stable slim

The image is Debian 12. Build and runtime stages use `debian:bookworm-slim`, and the runtime library names (`libavcodec59` and the rest) are bookworm names. Move those stages to the current Debian stable slim image, which is Trixie, and update the package names. Revisit `rust:1.89-bookworm` in the same change if a matching builder image exists.

### Statistics card

Done. The card shows packet rate, PTP sample, CPU, memory, send lateness, jitter, and the protocol counters. Each reading has a description. CPU and memory are the container when its cgroup counters exist, and this process otherwise. Send lateness is how long the latest due packet waited past its slot. Stream duration is on the Session card. Sync offset and WLED destinations stay on Control and Configuration.

### Special cases and troubleshooting

Write an operator document for cases that already bite people and are only partly described in the README:

- A multi-homed Docker host: which NIC carries HTTP, mDNS and AirPlay, unicast, and multicast, and what fails when UDP 11988 or `239.0.0.1` leaves the wrong interface.
- IPv6: `WLEDPLAY_IPV4_ONLY`, Avahi `use-ipv6`, and what a dual-stack host advertises.

### Apple devices with no Apple TV interface

Document the Auto Join limit that is only a row in `docs/SHAIRPORT_INTEGRATION.md` today. pyatv adds an output by talking to a group leader that exposes an Apple TV-style control interface. An iPhone, iPad, or Mac that is only the source of the stream does not expose that interface, so Auto Join cannot ask it to add WLEDPlay. Playing directly to WLEDPlay still works. HomePod and Apple TV leaders are the case Auto Join is built for.

### Why WLEDPlay cannot run on macOS

Write the technical reasons in one place. The short version already in the README is that NQPTP must bind UDP 319 and 320, and macOS already owns those ports for its own AirPlay stack. The write-up should also cover upstream Shairport Sync AirPlay 2 being a Linux host, Avahi versus mDNSResponder, and Docker Desktop on Mac not sharing host networking, mDNS, or multicast with the LAN. The analyzer binary can still read a PCM file on macOS. The receiver stack cannot.
