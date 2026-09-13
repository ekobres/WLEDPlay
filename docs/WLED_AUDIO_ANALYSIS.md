# WLED Audio Analysis Pipeline

**Authoritative source:** [wled/WLED `usermods/audioreactive/audio_reactive.cpp`](https://github.com/wled/WLED/blob/v16.0.1/usermods/audioreactive/audio_reactive.cpp) @ **v16.0.1**

WLEDPlay reproduces this pipeline for digital PCM input resampled to WLED's internal sample rate.

---

## Constants (ESP32 default build)

| Constant | Value | Line context |
|----------|-------|--------------|
| `SAMPLE_RATE` | **22050** Hz | `constexpr SRate_t SAMPLE_RATE = 22050` |
| `samplesFFT` | **512** (power of 2) | FFT batch size |
| `samplesFFT_2` | **256** | Meaningful FFT bins |
| `NUM_GEQ_CHANNELS` | **16** | GEQ output bands |
| `FFT_MIN_CYCLE` | **21** ms | Minimum FFT task period |
| `FFT_PREFER_EXACT_PEAKS` | defined | Blackman-Harris window |
| `FFT_DOWNSCALE` | **0.40** (BH window) | Magnitude calibration |
| `FFTScalingMode` | **3** (default) | Square-root scaling to 0–255 |
| `soundSquelch` | **10** (default) | Noise gate threshold |
| `sampleGain` | **60** (default) | Gain preset |
| `inputLevel` | **128** | UI level (0–255) |
| `soundAgc` | **0** (off default) | 1=normal, 2=vivid, 3=lazy |
| `limiterOn` | **true** (ESP32) | Dynamics limiter on volumeSmth |
| `decayTime` | **1400** ms | GEQ fall smoothing |
| `useMicFilter` | **true** at I2S init | 90 Hz–20 kHz bandpass (optional) |
| `useBandPassFilter` | **false** | Alternate band mapping |

Bin frequency: `f = bin_index * (SAMPLE_RATE / samplesFFT)` → **43.066 Hz per bin** at 22050/512.

---

## Pipeline overview

```text
PCM samples (22050 Hz mono int16/float)
  → getSample()           // DC leveling, exp filter, squelch, gain, sampleAvg
  → agcAvg()              // optional PI AGC
  → FFT batch (512)       // every ≥21 ms, if noise gate open (sampleAvg > 0.25)
      → optional runMicFilter()
      → DC removal
      → Blackman-Harris window
      → FFT → magnitude
      → zero DC bin
      → FFT_MajorPeak / FFT_Magnitude
  → band mapping (fftAddAvg ranges)
  → postProcessFFTResults()
  → fftResult[16]
  → detectSamplePeak()
  → transmitAudioData()   // every 20 ms
```

---

## Sample path (`getSample`)

Key steps:

1. **DC level:** `micLev` exponential tracker; subtract from sample.
2. **Envelope:** `expAdjF = 0.2 * |micInNoDC| + 0.8 * expAdjF`
3. **Squelch:** zero if `expAdjF <= soundSquelch` (special case when squelch=0 and <0.25)
4. **Gain:** `sampleAdj = tmpSample * sampleGain/40 * inputLevel/128 + tmpSample/16`
5. **sampleRaw** = clamped int16 of sampleAdj; **sampleReal** = tmpSample (pre-gain envelope)
6. **sampleAvg** = `(sampleAvg * 15 + sampleAdj) / 16`
7. **Peak (volume):** if `(binNum < 12 || maxVol < 1) && sampleAvg > 1` and 80 ms since last peak → `samplePeak`, `udpSamplePeak`

Default peak uses volume path (`binNum=8`, `maxVol=31`); FFT bin peak uses `valFFT[binNum] > maxVol`.

---

## AGC (`agcAvg`) — when `soundAgc > 0`

Presets (index = soundAgc - 1): normal, vivid, lazy.

PI controller targets:

- `sampleMax` tracks recent peak (5 s window via `maxSample5sec` / decay)
- Setpoints: ~60% and ~85% of max (`agcTarget0`, `agcTarget1`)
- `multAgc` clamped to `[1/64, 32]`
- Outputs: `rawSampleAgc`, smoothed `sampleAgc`

When AGC off, packet uses `sampleRaw` / `sampleAvg`.

---

## FFT

### Preprocessing

- Skip samples outside `[INT16_MIN+1024, INT16_MAX-1024]` for peak tracking
- Optional `runMicFilter`: IIR highpass ~90 Hz + FIR lowpass ~20 kHz

### Window & transform

With `FFT_PREFER_EXACT_PEAKS`:

- **Window:** Blackman-Harris (`FFT.windowing(Blackman_Harris)` or `dsps_wind_blackman_harris_f32`)
- **DC removal:** mean subtraction
- **FFT:** ArduinoFFT or ESP-DSP `dsps_fft2r_fc32`
- **Magnitude:** `complexToMagnitude` or `sqrt(re²+im²)`
- **DC bin:** forced to 0 after FFT

### Dominant frequency

- `FFT.majorPeak()` or max magnitude bin: `FFT_MajorPeak = i * (SAMPLE_RATE/samplesFFT)`
- Clamped to **1.0–11025.0** Hz

### Noise gate (FFT skip)

- FFT runs when `sampleAvg > 0.25`
- Band mapping uses `|sampleAvg| > 0.5`; else decay `fftCalc[i] *= 0.85`, zero if `< 4`

---

## Band mapping (default, `useBandPassFilter == false`)

Averages of FFT magnitude bins via `fftAddAvg(from, to)`:

| Band | Bin range | ~Frequency (Hz) |
|------|-----------|-----------------|
| 0 | 1–2 | 43–86 |
| 1 | 2–3 | 86–129 |
| 2 | 3–5 | 129–216 |
| 3 | 5–7 | 216–301 |
| 4 | 7–10 | 301–430 |
| 5 | 10–13 | 430–560 |
| 6 | 13–19 | 560–818 |
| 7 | 19–26 | 818–1120 |
| 8 | 26–33 | 1120–1421 |
| 9 | 33–44 | 1421–1895 |
| 10 | 44–56 | 1895–2412 |
| 11 | 56–70 | 2412–3015 |
| 12 | 70–86 | 3015–3704 |
| 13 | 86–104 | 3704–4479 |
| 14 | 104–165 × 0.88 | 4479–7106 |
| 15 | 165–215 × 0.70 | 7106–9259 |

`fftAddAvg`: sum bins `[from..to]`, multiply sum by **0.0625**, divide by count.

---

## Post-processing (`postProcessFFTResults`)

Per band:

1. Multiply by **pink noise curve** `fftResultPink[i]`
2. Multiply by `FFT_DOWNSCALE` if scaling mode > 0
3. Apply AGC multiplier or manual gain
4. Smooth into `fftAvg` (asymmetric rise/fall using `decayTime`)
5. Select `fftCalc` or `fftAvg` based on `limiterOn`
6. **Scaling mode 3 (default):** sqrt scaling mapped to 0–255
7. Optional post-gain from `inputLevel`
8. `fftResult[i] = constrain(currentResult, 0, 255)`

Pink curve:

```text
{1.70, 1.71, 1.73, 1.78, 1.68, 1.56, 1.55, 1.63,
 1.79, 1.62, 1.80, 2.06, 2.47, 3.35, 6.83, 9.55}
```

Sqrt scaling (mode 3):

```text
currentResult *= 0.38
currentResult -= 6.0
if > 1: currentResult = sqrt(currentResult)
currentResult *= 0.85 + (i / 4.5)
map [0, 16] → [0, 255]
```

---

## Peak detection (`detectSamplePeak`)

FFT-based (default with `maxVol > 0`):

```cpp
if (sampleAvg > 1 && maxVol > 0 && binNum > 4 &&
    valFFT[binNum] > maxVol && millis() - timeOfPeak > 100)
```

Sets `samplePeak` and `udpSamplePeak`. `udpSamplePeak` cleared on transmit.

---

## WLEDPlay adaptations for digital PCM

| WLED mic path | WLEDPlay digital path |
|---------------|----------------------|
| I2S @ 22050 Hz | Resample AirPlay 44100 stereo → 22050 mono |
| `micDataReal` from ADC/I2S | Normalized float from S16 PCM |
| `micLev` DC tracker | Retained (handles DC offset in stream) |
| `useMicFilter` default on | **Off** by default for digital (configurable) |

Document any measurable deviation during integration tests.

---

## References

- `FFTcode()`, `getSample()`, `agcAvg()`, `postProcessFFTResults()`, `detectSamplePeak()`
- Tag: **v16.0.1**, file: `usermods/audioreactive/audio_reactive.cpp`
