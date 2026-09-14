# WLEDPlay

## 1. Project Overview

**WLEDPlay** is an open-source, headless AirPlay-to-WLED audio-reactive bridge.

WLEDPlay presents itself on the local network as an AirPlay audio receiver. When selected as an AirPlay destination, it receives and decodes the audio stream, analyzes the decoded PCM audio in real time, and transmits audio-reactive analysis data to one or more WLED instances using WLED's Audio Sync protocol.

WLEDPlay does **not** require an audio output device and does not need to reproduce the received audio. Its primary purpose is to turn an AirPlay stream into synchronized WLED audio-reactive data.

The intended deployment is a lightweight Linux service, Docker container, LXC container, Raspberry Pi application, or similar always-on network service.

---

# 2. Primary Use Case

A user playing Apple Music from an iPhone, iPad, or Mac should see WLEDPlay as an available AirPlay destination.

For example:

```text
AirPlay destinations

✓ Living Room
✓ Kitchen
✓ WLEDPlay
```

The audible AirPlay destinations reproduce the music normally.

WLEDPlay receives the same AirPlay stream but instead performs:

```text
AirPlay 2
   │
   ▼
Shairport Sync
   │
   ▼
Decoded PCM
   │
   ▼
WLEDPlay Analyzer
   │
   ├── RMS / volume
   ├── peak detection
   ├── FFT
   ├── frequency-band mapping
   └── dominant frequency
   │
   ▼
WLED Audio Sync
   │
   ▼
UDP multicast / unicast
   │
   ├── WLED controller
   ├── WLED controller
   └── WLED controller
```

WLED instances configured to receive Audio Sync data should react to the music as though their Audio Reactive subsystem were receiving locally analyzed microphone/line-in audio.

---

# 3. Design Goals

WLEDPlay should:

1. Provide an AirPlay receiver specifically optimized for WLED audio-reactive visualization.
2. Require no microphone or physical audio interface.
3. Analyze the original digital audio stream rather than room audio.
4. Support multiple WLED controllers simultaneously.
5. Support **AirPlay 2** with NQPTP as the only AirPlay receiver mode.
6. Work as an AirPlay 2 destination in a multi-room AirPlay group alongside live speakers.
7. Produce WLED Audio Sync data closely equivalent to WLED's native audio-reactive analysis.
8. Run headlessly.
9. Be easy to deploy using Docker.
10. Have minimal runtime dependencies beyond the AirPlay receiver implementation.
11. Avoid maintaining an independent AirPlay protocol implementation.
12. Be suitable for eventual publication as a general-purpose open-source project.

---

# 4. Non-Goals

The initial implementation should NOT attempt to:

- implement AirPlay/AirPlay 2 from scratch;
- replace Shairport Sync;
- implement an AirPlay sender;
- support **AirPlay 1** (classic AirPlay without NQPTP). AirPlay 1 is deliberately excluded because WLEDPlay cannot join a live speaker group with correct presentation-time sync; the primary use case requires AirPlay 2 multi-room grouping with HomePods, Apple TVs, and other synchronized receivers;
- act as a general-purpose media server;
- provide whole-home audio distribution;
- control WLED effects;
- stream individual LED pixel values;
- replace WLED's rendering engine;
- decode Apple Music files directly;
- interact with Apple Music APIs;
- circumvent DRM;
- require WLED firmware modifications.

WLED remains responsible for rendering effects.

WLEDPlay supplies only the audio-analysis information consumed by WLED Audio Reactive effects.

---

# 5. AirPlay Receiver

## 5.1 Shairport Sync and NQPTP

The AirPlay implementation is **Shairport Sync** built with AirPlay 2 support, plus **NQPTP** for synchronized multi-room timing.

WLEDPlay must use an upstream Shairport Sync release rather than fork or embed substantial portions of its AirPlay implementation.

Shairport Sync must:

- advertise the configured WLEDPlay receiver through Bonjour/mDNS;
- receive AirPlay 2 audio;
- decode the stream to PCM;
- expose PCM to the WLEDPlay analyzer through stdout, FIFO, pipe, or another efficient local IPC mechanism.

NQPTP must run alongside Shairport Sync on the same host (UDP ports 319/320). macOS cannot host this stack; deployment targets Linux (Docker, bare metal, LXC, Pi).

WLEDPlay must treat Shairport Sync and NQPTP as replaceable external components.

The analyzer architecture must not depend unnecessarily on Shairport-specific internal APIs.

---

# 6. PCM Input

The analyzer must accept raw PCM audio.

The PCM input abstraction should allow future sources other than Shairport Sync.

At minimum, support:

```text
stdin
```

Preferably also support:

```text
named pipe / FIFO
```

The PCM subsystem must know or be configured with:

- sample rate;
- bit depth/sample format;
- channel count;
- byte order.

The analyzer should internally convert audio into a consistent floating-point representation.

Stereo sources should normally be downmixed to mono for WLED analysis.

The architecture should not preclude future stereo-aware visualization.

---

# 7. Audio Analysis

This is a critical component.

The objective is not merely to produce visually plausible FFT values. The objective is to reproduce, as closely as practical, the audio-analysis behavior expected by current WLED Audio Reactive effects.

## 7.1 Analysis Outputs

For each analysis interval calculate the information required by the current WLED Audio Sync protocol, including as applicable:

- instantaneous audio level;
- smoothed audio level;
- peak detection;
- FFT magnitude;
- 16 frequency-band values;
- dominant frequency;
- any other fields required by the supported WLED Audio Sync packet version.

Do not assume these fields or their encoding.

Determine them from the current upstream WLED implementation.

---

# 8. FFT Processing

Implement a proper FFT analysis pipeline.

Expected processing stages include:

```text
PCM
 ↓
mono conversion
 ↓
DC removal if required
 ↓
window function
 ↓
FFT
 ↓
magnitude calculation
 ↓
frequency mapping
 ↓
16 WLED bands
 ↓
normalization / AGC
 ↓
smoothing
 ↓
WLED Audio Sync packet
```

Do NOT simply take the first 16 FFT bins.

Frequency bands should reproduce WLED's own band mapping as closely as possible.

Before implementing this subsystem, inspect the current WLED Audio Reactive source code and document:

- FFT size;
- window function;
- frequency-band boundaries;
- magnitude calculation;
- noise filtering;
- gain behavior;
- AGC behavior;
- smoothing;
- peak detection;
- dominant-frequency calculation;
- output scaling.

Where exact reproduction is impractical, document the deviation.

---

# 9. WLED Audio Sync Protocol

WLEDPlay must implement the current WLED Audio Sync network protocol.

Do not rely solely on third-party descriptions of the packet format.

Use the current upstream WLED source code as the authoritative specification.

The implementation should:

- encode packets exactly as expected by WLED;
- support the current protocol/version used by released WLED firmware;
- validate packet structure against WLED source;
- clearly isolate protocol serialization from audio analysis.

Create a module approximately equivalent to:

```text
WLEDAudioSyncEncoder
```

so future protocol versions can be added independently.

---

# 10. Network Transmission

Default transmission should use WLED's standard Audio Sync multicast configuration.

Expected default:

```text
Multicast address: 239.0.0.1
UDP port:          11988
```

These defaults MUST be verified against current upstream WLED before implementation.

Both address and port must be configurable.

---

# 11. Multicast and Unicast

Support:

### Multicast

One packet stream supplies any number of listening WLED controllers.

Example:

```text
WLEDPlay
   │
   └── 239.0.0.1:11988
            │
            ├── WLED #1
            ├── WLED #2
            └── WLED #3
```

### Unicast

Allow explicit destination addresses.

Example:

```yaml
wled:
  destinations:
    - 10.0.20.41
    - 10.0.20.42
```

Multicast should remain the default and must be toggleable at runtime from the web UI without removing configured unicast destinations.

---

# 12. Update Frequency

WLEDPlay should transmit analysis data at a rate compatible with WLED Audio Reactive.

Initial target:

```text
50 updates/second
```

or one update approximately every:

```text
20 ms
```

The actual rate should be derived from upstream WLED behavior and made configurable if useful.

Do not transmit packets unnecessarily faster than WLED can consume them.

---

# 13. Synchronization

AirPlay 2 introduces buffering and presentation timing shared with grouped speakers via NQPTP.

WLEDPlay compensates using two independent operator settings:

- `audio.shairport_latency_offset_s` — Shairport `audio_backend_latency_offset_in_seconds` (negative = earlier PCM from Shairport). **Restart** to apply. The absolute value is the early-PCM margin (e.g. `-1.2` → 1200 ms lead).
- `audio.sync_offset_ms` — unsigned hold after decode before each WLED UDP packet (**live**). `0` = send as soon as PCM arrives (up to the Shairport lead early). Matching the lead in ms (e.g. `1200` when lead is `-1.2`) = on time at presentation. Higher = later LEDs.

The operator console exposes both fields separately in yaml and the web UI.

The system distinguishes:

```text
audio received
audio decoded
audio analyzed
audio presentation time (AirPlay 2 group clock)
```

The ideal behavior is:

> WLED effects react when the corresponding audio is actually heard from synchronized AirPlay speakers.

Synchronization accuracy must be measured and documented (`docs/SYNC_MEASUREMENT.md`).

Example:

```yaml
audio:
  sync_offset_ms: 1200
  shairport_latency_offset_s: -1.2
```

Send delay is adjustable live while a stream is active; Shairport lead changes require restart (see §22.2).

---

# 14. AirPlay Multi-Room Operation

A primary use case is:

```text
iPhone
  │
  ├── AirPlay speaker
  ├── AirPlay speaker
  └── WLEDPlay
```

WLEDPlay must participate as an AirPlay 2 receiver while intentionally producing no audible output.

The implementation must be tested as part of an AirPlay 2 multi-room group with at least one audible AirPlay 2 speaker.

Synchronization with other AirPlay 2 receivers in the group is required for the primary use case.

---

# 15. AirPlay Receiver Name

The advertised receiver name must be configurable.

Default:

```text
WLEDPlay
```

Example:

```yaml
airplay:
  name: "WLED Visualizer"
```

The operator must be able to rename the instance from the web UI. Changing the advertised AirPlay name may restart Shairport Sync so Bonjour re-publishes the new name. An in-progress AirPlay session may drop; that is acceptable if the UI warns before applying.

Multiple independent WLEDPlay instances (e.g. Living Room, Patio) are supported by running separate processes or containers with distinct `airplay.name` values and WLED destination lists.

---

# 16. Deployment

## 16.1 Native Linux

WLEDPlay should run as a normal Linux daemon.

Provide a systemd service definition.

Expected environments include:

- Debian;
- Ubuntu;
- Raspberry Pi OS;
- Proxmox LXC.

---

# 17. Docker

Docker should be a first-class deployment target.

A normal deployment should require approximately:

```yaml
services:
  wledplay:
    image: ghcr.io/<project>/wledplay:latest
    container_name: wledplay
    network_mode: host
    restart: unless-stopped
    environment:
      WLEDPLAY_AIRPLAY_NAME: "WLEDPlay"
```

Host networking is acceptable and likely preferable because WLEDPlay depends on:

- Bonjour/mDNS;
- AirPlay;
- UDP multicast;
- AirPlay 2 timing services.

Multiple independent instances (different `airplay.name` and WLED destinations) are supported by running separate containers or processes.

The container should NOT require:

```text
/dev/snd
```

or any physical audio hardware.

---

# 18. Container Processes

The container will likely contain:

```text
nqptp
shairport-sync
wledplay
```

Use an appropriate lightweight process supervisor or entrypoint.

The container must correctly handle:

- startup ordering;
- SIGTERM;
- graceful shutdown;
- child-process failure;
- automatic restart behavior.

Avoid unnecessary init-system complexity inside the container.

---

# 19. Configuration

Support configuration through a YAML file.

Example:

```yaml
airplay:
  name: "WLEDPlay"

audio:
  sync_offset_ms: 0
  gain: auto

wled:
  multicast_address: "239.0.0.1"
  port: 11988

  destinations: []

network:
  http: ""
  airplay: ""
  unicast: ""
  multicast: ""

logging:
  level: info

web:
  enabled: true
  listen: "0.0.0.0"
  port: 8080
```

Docker environment variables should override common settings.

For example:

```text
WLEDPLAY_AIRPLAY_NAME
WLEDPLAY_MULTICAST_ADDRESS
WLEDPLAY_PORT
WLEDPLAY_SYNC_OFFSET_MS
WLEDPLAY_LOG_LEVEL
WLEDPLAY_HTTP_INTERFACE
WLEDPLAY_AIRPLAY_INTERFACE
WLEDPLAY_UNICAST_INTERFACE
WLEDPLAY_MULTICAST_INTERFACE
WLEDPLAY_DESTINATION
```

Settings changed from the web UI must persist across process restart when a writable configuration file is available. Docker deployments should mount that file (or an overlay) rather than relying on the image-baked copy.

If the configuration path is read-only, apply the change in memory and report that it will revert on restart.

Environment variables remain the override at process start. After a UI persist + restart, the saved YAML is the source of truth unless an env var is still set.

---

# 20. Logging

Use structured, useful logging.

Normal startup should report approximately:

```text
WLEDPlay 0.x.x

AirPlay receiver: WLEDPlay
PCM: 48000 Hz / stereo / 16-bit
Analyzer: initialized
WLED Audio Sync: protocol vN
Destination: 239.0.0.1:11988
Update rate: 50 Hz

Waiting for AirPlay stream...
```

On connection:

```text
AirPlay stream connected
Source: <if available>
Sample rate: 48000 Hz
Channels: 2
Audio analysis started
```

Avoid logging every audio packet.

Debug logging may expose:

- RMS;
- peak state;
- dominant frequency;
- FFT bands;
- packet counters;
- dropped frames;
- timing offset.

---

# 21. Status Interface

A simple HTTP status endpoint is required.

Minimum read endpoints:

```text
GET /health
GET /status
GET /config
```

`/health` should be appropriate for container health checks.

`/status` should return JSON containing runtime information such as:

```json
{
  "version": "0.1.0",
  "airplay": {
    "connected": true,
    "receiver_name": "WLEDPlay"
  },
  "audio": {
    "sample_rate": 48000,
    "channels": 2,
    "rms": 0.42,
    "peak": false,
    "dominant_frequency": 118.3
  },
  "wled": {
    "packets_per_second": 50,
    "destination": "239.0.0.1:11988"
  }
}
```

`/config` should return the editable operator settings and enough context to render controls (available network interfaces, whether multicast is enabled, current delay, destinations, persistence state). Exact schema may evolve.

Control endpoints are specified in §22.2.

---

# 22. Web UI

A browser UI is required after the core audio path works.

WLEDPlay must function fully without the UI (`--no-web` or `web.enabled: false`). The audio pipeline must not depend on a connected browser.

## 22.1 Display

The UI should display:

- AirPlay connection state;
- current track metadata and artwork if available;
- audio level;
- 16 FFT bands at the analysis/send frame rate;
- dominant frequency;
- packet rate;
- WLED destinations;
- multicast on/off;
- current HTTP, AirPlay/mDNS, unicast, and multicast interfaces;
- synchronization (Shairport lead + send delay);
- AirPlay receiver name.

## 22.2 Live control

The UI is the primary operator console for a running instance. To the extent a setting can be applied without breaking the AirPlay or WLED path, it must be changeable live. Settings that require a subsystem restart must still be changeable from the UI, with an explicit confirm step.

### Required controls

| Control | Expected behavior |
|---|---|
| Visual delay | Advance or retard visualization relative to the speakers. Apply immediately during an active stream. Support both earlier and later visuals (negative and positive compensation). |
| AirPlay name | Rename the advertised receiver from the default `WLEDPlay`. Re-advertise via mDNS. Warn that the current AirPlay session may drop. |
| HTTP interface | Bind the control UI to a management NIC, or leave unset for all interfaces. Apply via stack restart. |
| AirPlay / mDNS interface | Bind Shairport Sync and Avahi to the user-network NIC. Apply via stack restart. |
| Unicast interface | Bind WLED Audio Sync unicast UDP to the IoT / device NIC. Apply via stack restart. |
| Multicast interface | Bind WLED Audio Sync multicast (`239.0.0.1`) to an IoT NIC, which may differ from unicast. Apply via stack restart. |
| Multicast | Enable or disable WLED Audio Sync multicast send/join without stopping unicast destinations. Apply immediately. |
| Unicast destinations | Add or remove WLED controller IPs (and optional port). Apply immediately. An empty list with multicast off means “send nowhere.” |
| Restart | Restart the WLEDPlay process (and, in the Docker stack, Shairport / NQPTP as needed) so pending bind/name/interface changes take effect. Docker `restart: unless-stopped` (or equivalent) must bring the stack back. |

### Additional controls (should)

- WLED UDP port
- Multicast group address
- Shairport lead (`shairport_latency_offset_s`)
- send delay (`sync_offset_ms`)
- AGC / gain, if changing it does not require rebuilding the analyzer mid-buffer

### Apply vs restart

Classify every control as:

```text
live     — takes effect on the next packet / immediately
restart  — requires process or Shairport restart
```

`GET /config` must mark each field with that classification.

Live examples: send delay, multicast enable, destination list.

Restart examples: Shairport lead, AirPlay name, any of the four network interfaces, HTTP listen address (do not offer HTTP bind/port changes that would disconnect the UI without a warning).

### Control API

Minimum write endpoints:

```text
GET    /config
PATCH  /config
POST   /restart
GET    /interfaces
```

`PATCH /config` accepts a partial JSON body of operator settings. Unknown fields are ignored. Invalid values return `4xx` and leave the previous settings unchanged.

`GET /interfaces` returns the host’s usable IPv4 interfaces (name, address). The four current selections are on `GET /config`.

`POST /restart` initiates an orderly shutdown after the current settings have been persisted (when possible). The container/service supervisor restarts the process.

The control API must not be required for audio-reactive operation.

### Persistence

Successful `PATCH /config` writes the new values to the configured YAML file when that path is writable. The response indicates `persisted: true|false`.

### Safety

- Do not interrupt PCM analysis to render the UI.
- A failed apply must leave the last good configuration in effect.
- Confirm destructive or session-dropping actions (rename, interface change, restart).
- The control surface is local-LAN only (see §32). No Internet callback, account, or cloud dashboard.

---

# 23. Metadata

When Shairport Sync is built with metadata support and a metadata pipe is configured, WLEDPlay must expose:

- artist;
- album;
- track;
- album artwork (`GET /artwork`, UI display).

Metadata is not required for audio-reactive operation but is implemented in v1.

Future metadata-driven features (WLED palette from artwork, track-dependent presets) remain in §39.

---

# 24. Reliability

WLEDPlay is intended to run continuously.

It must tolerate:

- AirPlay client disconnects;
- new AirPlay sessions;
- network interruptions;
- WLED controllers disappearing;
- multicast packet loss;
- malformed or incomplete PCM input;
- Shairport Sync restart;
- temporary absence of AirPlay sources.

A WLED controller becoming unreachable must never stop audio processing.

UDP transmission errors should be logged appropriately but should not terminate the daemon.

---

# 25. Performance

WLEDPlay should have modest resource requirements.

The audio pipeline should avoid unnecessary allocations inside real-time processing loops.

Target environments include Raspberry Pi-class hardware.

The application should comfortably perform:

- PCM decoding handoff;
- FFT;
- 16-band analysis;
- 50-Hz WLED packet generation

using a small fraction of a modern CPU core.

---

# 26. Implementation Language

Preferred analyzer implementation languages:

1. **Rust**
2. C++
3. C

Rust is preferred for the new WLEDPlay-specific code because it provides:

- strong memory safety;
- good networking support;
- efficient numerical processing;
- straightforward static binaries;
- good container deployment;
- good concurrency primitives.

Do not rewrite Shairport Sync in Rust.

Use the mature upstream AirPlay implementation.

---

# 27. Internal Architecture

Prefer clear subsystem boundaries.

Suggested layout:

```text
wledplay/
├── src/
│   ├── main.rs
│   ├── config.rs
│   ├── pcm/
│   │   ├── mod.rs
│   │   └── stdin.rs
│   ├── audio/
│   │   ├── mod.rs
│   │   ├── fft.rs
│   │   ├── bands.rs
│   │   ├── agc.rs
│   │   └── peak.rs
│   ├── wled/
│   │   ├── mod.rs
│   │   ├── protocol.rs
│   │   └── sender.rs
│   ├── timing/
│   │   └── mod.rs
│   ├── metadata/
│   │   └── mod.rs
│   └── web/
│       └── mod.rs
│
├── tests/
├── docker/
├── config/
├── docs/
├── Dockerfile
├── docker-compose.yml
├── Cargo.toml
├── LICENSE
└── README.md
```

This is a recommendation, not a rigid requirement.

---

# 28. Testing

Automated tests are required.

## Protocol tests

Create known WLED Audio Sync packet vectors and verify byte-for-byte encoding.

Where practical, derive expected packets from upstream WLED.

## FFT tests

Use generated signals:

```text
100 Hz sine
440 Hz sine
1 kHz sine
10 kHz sine
white noise
silence
impulse
```

Verify that energy appears in the expected WLED bands.

## Level tests

Verify:

- silence;
- constant sine;
- clipping;
- gradual volume increase;
- transient peaks.

## PCM tests

Verify:

- stereo downmix;
- endian conversion;
- sample conversion;
- partial buffers;
- stream interruption.

## Network tests

Verify multicast and unicast transmission.

---

# 29. WLED Compatibility Test

Create a repeatable integration test involving an actual WLED instance.

Configure WLED:

```text
Audio Reactive:
    Audio Source = Receive
```

Play known test audio through WLEDPlay and compare the resulting Audio Reactive data against the same audio analyzed locally by WLED where practical.

The goal is behavioral compatibility, not merely packet acceptance.

---

# 30. Reference Implementations

Before implementing the audio-analysis and protocol layers, study:

### WLED

Use the current upstream WLED Audio Reactive implementation as the authoritative source for:

- Audio Sync protocol;
- FFT processing;
- frequency bands;
- AGC;
- smoothing;
- peak detection.

Do not assume third-party documentation is current.

### Shairport Sync

Use upstream Shairport Sync for:

- AirPlay reception;
- AirPlay 2 support;
- PCM output;
- timing;
- metadata.

Avoid modifying Shairport Sync unless absolutely necessary.

### ESP Air Sync

Study ESP Air Sync as an existing implementation of:

```text
AirPlay → PCM → FFT → WLED Audio Sync
```

Determine:

- how it maps FFT data to WLED;
- which WLED protocol version it implements;
- how it handles scaling;
- how it handles timing;
- whether any implementation can legally and technically be reused.

Do not copy code unless its license is compatible with WLEDPlay.

### Feed_My_WLED

Use Feed_My_WLED as another reference for WLED packet generation and interoperability.

Do not assume its FFT implementation accurately reproduces WLED's native analysis.

---

# 31. Licensing

Before selecting the WLEDPlay license, document the licenses of:

- WLED;
- Shairport Sync;
- NQPTP;
- ESP Air Sync;
- any FFT library;
- any copied or derived source code.

Prefer architectural separation that avoids unnecessarily imposing upstream copyleft requirements on WLEDPlay-specific code.

Do not copy GPL code into the WLEDPlay executable without first understanding the licensing implications.

The Docker image may package separately licensed executables.

Maintain required license notices.

---

# 32. Security

WLEDPlay should not require Internet access after installation.

The normal data path should remain entirely local:

```text
AirPlay client
      ↓
local LAN
      ↓
WLEDPlay
      ↓
local LAN
      ↓
WLED
```

Do not implement telemetry.

Do not transmit:

- audio;
- metadata;
- usage statistics

outside the local network.

Any future telemetry must be explicit opt-in.

The HTTP control API (§22.2) is reachable by any client that can open the listen address. v1 assumes a trusted LAN. Do not bind the control UI to the public Internet. Optional shared-secret or listen-address restriction may be added later; it is not required for the initial control surface.

---

# 33. Privacy

Decoded audio should exist only transiently in memory.

WLEDPlay should not record or persist PCM audio.

Do not write audio streams to disk.

Debugging facilities that dump PCM must be explicitly enabled and clearly documented.

---

# 34. Graceful Silence Behavior

When no AirPlay stream is active:

- stop FFT processing;
- stop normal Audio Sync transmission, unless WLED requires periodic idle packets;
- consume minimal CPU;
- remain advertised as an AirPlay receiver.

When playback pauses, behavior should correctly communicate silence to WLED so effects do not remain stuck at their last reactive state.

Determine the appropriate behavior experimentally and from WLED source.

---

# 35. Command-Line Interface

Provide a useful CLI.

Examples:

```bash
wledplay --config /etc/wledplay/config.yaml

wledplay \
  --input stdin \
  --multicast 239.0.0.1 \
  --port 11988

wledplay --version

wledplay --help
```

A useful development mode should allow PCM files or generated test audio to bypass AirPlay entirely.

Example:

```bash
cat test.pcm | wledplay --input stdin
```

This is important for deterministic testing.

---

# 36. Observability

Maintain counters for:

- PCM frames received;
- FFT frames processed;
- WLED packets transmitted;
- UDP errors;
- PCM underruns;
- analysis overruns;
- AirPlay connections;
- stream duration.

These should be available through the status interface.

Prometheus metrics may be added later but are not required for v1.

---

# 37. v1 Scope (implemented)

The v1 product includes everything specified in the body of this document:

- PCM stdin/FIFO and file input for development (§6);
- WLED-compatible analysis and Audio Sync V2 transmission (§7–§12);
- AirPlay 2 receiver via Shairport Sync + NQPTP (§5, §14);
- Docker host-network deployment (§17);
- HTTP status, debug UI, and operator console (§21–§22);
- Shairport metadata and artwork (§23);
- Per-traffic network interface selection (§22.2);
- Research and integration docs (§40).

Deferred work is listed in §39. Operational acceptance criteria remain in §38.

---

# 38. Acceptance Criteria for v1.0

WLEDPlay v1.0 is complete when all of the following are true:

1. A standard iPhone/iPad/Mac sees WLEDPlay as an AirPlay destination.
2. Apple Music can stream to it without special client software.
3. WLEDPlay requires no microphone.
4. WLEDPlay requires no physical audio output device.
5. Received audio is decoded to PCM.
6. PCM is analyzed in real time.
7. WLED-compatible 16-band FFT data is generated.
8. Audio level and peak information are generated.
9. WLED Audio Sync packets are transmitted.
10. Current released WLED firmware accepts the packets.
11. Multiple WLED controllers can consume the same multicast stream.
12. WLED Audio Reactive effects respond appropriately.
13. WLEDPlay can participate in an AirPlay multi-room session.
14. Visual response is reasonably synchronized with audible AirPlay playback.
15. WLEDPlay runs unattended for at least 24 hours through multiple playback/disconnect cycles.
16. Docker deployment requires no sound hardware.
17. No audio data leaves the local network.
18. No audio is persistently recorded.
19. Automated protocol and audio-analysis tests pass.
20. Installation and configuration are documented.

---

# 39. Future Features

Explicitly defer these until after the core system is stable:

### Album-art palette generation

```text
AirPlay metadata
      ↓
album artwork
      ↓
dominant colors
      ↓
WLED palette
```

### Automatic WLED discovery

Discover WLED controllers using mDNS.

### WLED REST integration

Allow WLEDPlay to optionally:

- enable Audio Receive mode;
- select presets;
- select palettes;
- restore previous state when playback stops.

### Home Assistant integration

Expose:

- playback state;
- track;
- artist;
- audio level;
- WLEDPlay status.

### MQTT

Publish stream and metadata information.

### Prometheus

Expose operational metrics.

### Audio passthrough

Optionally reproduce or forward received audio.

This should not compromise synchronization or the headless analyzer-only use case.

---

# 40. Documentation

Upstream research and integration notes live in:

```text
docs/
├── WLED_AUDIO_SYNC.md
├── WLED_AUDIO_ANALYSIS.md
├── SHAIRPORT_INTEGRATION.md
└── LICENSING.md
```

These documents record byte-level protocol details, WLED analysis behavior, Shairport/NQPTP integration, and license compatibility. They are the authoritative reference for protocol and stack decisions.

---

# 41. Guiding Principle

When choosing between:

> something that merely makes LEDs react to music

and:

> something that behaves like WLED's native Audio Reactive source but receives pristine digital audio over AirPlay,

choose the latter.

WLEDPlay should be a **network-native digital audio source for WLED Audio Reactive**, not merely another music visualizer.