# WLED Audio Sync Protocol (V2)

**Authoritative source:** [wled/WLED `usermods/audioreactive/audio_reactive.cpp`](https://github.com/wled/WLED/blob/v16.0.1/usermods/audioreactive/audio_reactive.cpp)  
**Compatibility pin:** official WLED **v16.0.1** (16.x series; verify newer 16.x tags at implementation time)

This document describes the UDP Audio Sync format consumed by WLED Audio Reactive when **Sync Mode = Receive**.

---

## Transport

| Parameter | Default | Source |
|-----------|---------|--------|
| Protocol | UDP | `audio_reactive.cpp` |
| Destination | Multicast `239.0.0.1` | `connected()` → `fftUdp.beginMulticast(IPAddress(239, 0, 0, 1), audioSyncPort)` |
| Port | `11988` | `uint16_t audioSyncPort = 11988` |
| Send cadence | ~20 ms (50 Hz max) | `if ((audioSyncEnabled & 0x01) && (millis() - lastTime > 20)) transmitAudioData()` |
| Max packet size | 88 bytes (buffer) | `#define UDPSOUND_MAX_PACKET 88` |

Port is user-configurable in WLED usermod settings (`sync.port`). WLED documentation states external senders must not exceed 50 fps.

---

## Protocol versions

| Header (6 bytes, ASCII) | Struct | Size | Status |
|-------------------------|--------|------|--------|
| `"00001"` | `audioSyncPacket_v1` | 83 bytes payload (88 with compiler padding) | Legacy |
| `"00002"` | `audioSyncPacket` | **44 bytes** | **Current (V2)** |

Constants (PROGMEM):

```cpp
const char AudioReactive::UDP_SYNC_HEADER[] = "00002";
const char AudioReactive::UDP_SYNC_HEADER_v1[] = "00001";
```

WLEDPlay implements **V2 only** for v1.0.

---

## V2 packet layout (`audioSyncPacket`)

Packed struct, **44 bytes total**, little-endian on ESP32 (standard for IEEE floats in WLED).

| Offset | Size | Field | Type | Notes |
|--------|------|-------|------|-------|
| 0 | 6 | `header` | `char[6]` | Must be `"00002\0"` truncated to 6 chars (no null terminator required in compare; `strncmp` length 6) |
| 6 | 2 | `reserved1` | `uint8_t[2]` | Unused; zero |
| 8 | 4 | `sampleRaw` | `float` | If AGC off: `sampleRaw` (int16 cast path). If AGC on: `rawSampleAgc` |
| 12 | 4 | `sampleSmth` | `float` | If AGC off: `sampleAvg`. If AGC on: `sampleAgc` |
| 16 | 1 | `samplePeak` | `uint8_t` | `0` = no peak, `>=1` = peak |
| 17 | 1 | `reserved2` | `uint8_t` | Unused; zero |
| 18 | 16 | `fftResult` | `uint8_t[16]` | GEQ band values 0–254 (transmitter clamps with `constrain(..., 0, 254)`) |
| 34 | 2 | `reserved3` | `uint16_t` | Compiler padding gap; zero |
| 36 | 4 | `FFT_Magnitude` | `float` | Peak FFT magnitude (AGC-scaled on transmit side when AGC enabled) |
| 40 | 4 | `FFT_MajorPeak` | `float` | Dominant frequency Hz, constrained 1.0–11025.0 on receive |

Source struct (v16.0.1):

```cpp
struct __attribute__ ((packed)) audioSyncPacket {
  char header[6];
  uint8_t reserved1[2];
  float sampleRaw;
  float sampleSmth;
  uint8_t samplePeak;
  uint8_t reserved2;
  uint8_t fftResult[16];
  uint16_t reserved3;
  float FFT_Magnitude;
  float FFT_MajorPeak;
};
```

---

## Transmission (`transmitAudioData`)

Function: `AudioReactive::transmitAudioData()` (ESP32 only)

1. Requires `udpSyncConnected`.
2. Zero-initializes entire struct (including padding).
3. Copies header `"00002"`.
4. Sets volume fields from local analysis state.
5. Sets `samplePeak` from `udpSamplePeak`, then clears `udpSamplePeak`.
6. Copies `fftResult[0..15]` clamped to 0–254.
7. Sets `FFT_Magnitude` = `my_magnitude`, `FFT_MajorPeak` = `FFT_MajorPeak`.
8. Sends via `fftUdp.beginMulticastPacket()` + `write()` + `endPacket()`.

---

## Reception (`receiveAudioData` / `decodeAudioData`)

Function: `AudioReactive::receiveAudioData()`

Validation:

- `packetSize == sizeof(audioSyncPacket)` (44) **and** header matches `"00002"` → V2
- Else if `packetSize == sizeof(audioSyncPacket_v1)` and header `"00001"` → V1

On V2 decode:

- `volumeSmth` / `volumeRaw` from packet floats (clamped ≥ 0)
- Internal `sampleRaw`, `sampleAvg`, `sampleAgc`, `multAgc = 1.0`
- `samplePeak` set only if not already true (animation hold)
- `fftResult[i]` copied directly
- `FFT_MajorPeak` constrained to 1.0–11025.0

When in **Receive mode** (`audioSyncEnabled & 0x02`), local FFT/mic processing is suspended (`disableSoundProcessing = true`).

---

## Sync mode bitmask

`audioSyncEnabled` (config `sync.mode`):

| Bit | Value | Meaning |
|-----|-------|---------|
| 0 | 1 | Send |
| 1 | 2 | Receive |

WLEDPlay acts as an external **sender** (bit 0). WLED controllers use Receive (bit 2).

---

## Idle / silence behavior

When no valid UDP packets arrive, receive-side effects may appear frozen. WLEDPlay should transmit zero-valued or decayed packets during silence so receivers stay responsive (see WLED source `decodeAudioData` and FFT noise-gate decay paths).

---

## References

- WLED docs: [Audio Reactive — Audio Sync](https://kno.wled.ge/advanced/audio-reactive/)
- Upstream file: `usermods/audioreactive/audio_reactive.cpp` @ tag **v16.0.1**
- Key functions: `transmitAudioData()`, `receiveAudioData()`, `decodeAudioData()`, `connected()`
