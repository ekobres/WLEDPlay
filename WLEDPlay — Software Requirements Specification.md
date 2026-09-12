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
5. Support AirPlay 2 where practical.
6. Work as an AirPlay destination in a multi-room AirPlay configuration.
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

## 5.1 Shairport Sync

The preferred AirPlay implementation is **Shairport Sync**.

WLEDPlay should use an upstream Shairport Sync release rather than fork or embed substantial portions of its AirPlay implementation.

Shairport Sync should:

- advertise the configured WLEDPlay receiver through Bonjour/mDNS;
- receive AirPlay audio;
- decode the stream to PCM;
- expose PCM to the WLEDPlay analyzer through stdout, FIFO, pipe, or another efficient local IPC mechanism;
- support AirPlay 2 when available and properly configured.

WLEDPlay should treat Shairport Sync as a replaceable external component.

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

Multicast should remain the default.

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

AirPlay introduces buffering and presentation timing.

WLEDPlay should investigate whether Shairport Sync exposes sufficient timing information to align WLED visualization with the actual AirPlay presentation time.

The system should distinguish between:

```text
audio received
audio decoded
audio analyzed
audio presentation time
```

The ideal behavior is:

> WLED effects react when the corresponding audio is actually heard from synchronized AirPlay speakers.

Do not artificially delay visualization unless timing information demonstrates that doing so improves synchronization.

For the initial implementation, synchronization accuracy should be measured and documented.

Future versions may implement configurable timing compensation.

Example:

```yaml
audio:
  sync_offset_ms: 0
```

Positive and negative offsets should eventually be supported.

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

WLEDPlay should participate as an AirPlay receiver while intentionally producing no audible output.

The implementation should be tested as part of an AirPlay multi-room group.

Synchronization with other AirPlay 2 receivers is highly desirable.

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
- AirPlay timing services.

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
```

---

# 20. Logging

Use structured, useful logging.

Normal startup should report approximately:

```text
WLEDPlay 0.x.x

AirPlay receiver: WLEDPlay
PCM: 44100 Hz / stereo / 16-bit
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
Sample rate: 44100 Hz
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

A simple HTTP status endpoint is desirable.

Minimum endpoints:

```text
GET /health
GET /status
```

`/health` should be appropriate for container health checks.

`/status` should return JSON containing information such as:

```json
{
  "version": "0.1.0",
  "airplay": {
    "connected": true,
    "receiver_name": "WLEDPlay"
  },
  "audio": {
    "sample_rate": 44100,
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

Exact schema may evolve.

---

# 22. Optional Web UI

A minimal browser UI may be implemented after the core functionality works.

It should display:

- AirPlay connection state;
- current track metadata if available;
- audio level;
- 16 FFT bands;
- dominant frequency;
- packet rate;
- WLED destinations;
- synchronization offset.

Avoid making the web interface a dependency of the audio pipeline.

WLEDPlay must function fully without the UI.

---

# 23. Metadata

If Shairport Sync provides metadata, WLEDPlay may expose:

- artist;
- album;
- track;
- album artwork.

Metadata is not required for audio-reactive operation.

Design the metadata interface so future functionality could use it.

Potential future features include:

- WLED palette derived from album artwork;
- track-dependent presets;
- artist/genre-dependent visualization.

These are explicitly outside the initial release.

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

# 37. Release Phases

## Phase 1 — Protocol Proof of Concept

Goal:

```text
PCM file/stdin
    ↓
WLEDPlay
    ↓
WLED Audio Sync
    ↓
real WLED controller
```

No AirPlay.

Success criterion:

Known audio produces correct Audio Reactive behavior on WLED.

---

## Phase 2 — Analysis Compatibility

Implement and validate:

- WLED frequency bands;
- AGC;
- smoothing;
- peak detection;
- dominant frequency.

Compare against WLED's native analysis.

---

## Phase 3 — AirPlay 1 Integration

Integrate Shairport Sync PCM output.

Goal:

```text
iPhone → AirPlay → WLEDPlay → WLED
```

Validate basic end-to-end operation before adding AirPlay 2 complexity.

---

## Phase 4 — AirPlay 2

Add:

- NQPTP;
- AirPlay 2 configuration;
- multi-room testing;
- synchronization measurement.

---

## Phase 5 — Docker

Produce a self-contained container containing:

```text
Shairport Sync
NQPTP
WLEDPlay
```

No audio device required.

---

## Phase 6 — Status/API

Implement:

```text
/health
/status
```

Add minimal visualization/debugging UI if useful.

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

### Multiple independent AirPlay receivers

Example:

```text
WLEDPlay - Living Room
WLEDPlay - Patio
WLEDPlay - Basement
```

each broadcasting to a different WLED Audio Sync multicast/unicast group.

### Audio passthrough

Optionally reproduce or forward received audio.

This should not compromise synchronization or the headless analyzer-only use case.

---

# 40. Initial Development Task for Cursor

Do **not** begin by implementing the entire system.

Start by researching the current upstream implementations and produce:

```text
docs/
├── WLED_AUDIO_SYNC.md
├── WLED_AUDIO_ANALYSIS.md
├── SHAIRPORT_INTEGRATION.md
└── LICENSING.md
```

### WLED_AUDIO_SYNC.md

Document the current WLED Audio Sync packet format byte-for-byte, including:

- protocol/version identification;
- field offsets;
- data types;
- endianness;
- packet size;
- multicast defaults;
- transmission frequency;
- compatibility/version handling.

Cite exact upstream source files/functions/commits.

### WLED_AUDIO_ANALYSIS.md

Document exactly how current WLED converts PCM/microphone samples into Audio Reactive data:

```text
samples
→ preprocessing
→ FFT
→ bands
→ AGC
→ smoothing
→ peak
→ network representation
```

Include constants and equations where applicable.

### SHAIRPORT_INTEGRATION.md

Determine:

- recommended Shairport Sync release;
- AirPlay 1 versus AirPlay 2 configuration;
- raw PCM output mechanism;
- PCM format;
- startup/shutdown behavior;
- NQPTP requirements;
- metadata interface;
- presentation timing information;
- Docker networking requirements.

### LICENSING.md

Determine license compatibility among all contemplated components.

Only after these documents are complete should implementation begin.

---

# 41. Guiding Principle

When choosing between:

> something that merely makes LEDs react to music

and:

> something that behaves like WLED's native Audio Reactive source but receives pristine digital audio over AirPlay,

choose the latter.

WLEDPlay should be a **network-native digital audio source for WLED Audio Reactive**, not merely another music visualizer.