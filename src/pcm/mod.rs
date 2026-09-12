use crate::config::PcmFormat;
use std::io::Read;

pub const ANALYSIS_RATE: u32 = 22050;

pub struct PcmDecoder {
    format: PcmFormat,
    pub resample_phase: f64,
    pub resample_ratio: f64,
}

impl PcmDecoder {
    pub fn new(input_rate: u32, _channels: u16, format: PcmFormat) -> Self {
        Self {
            format,
            resample_phase: 0.0,
            resample_ratio: input_rate as f64 / ANALYSIS_RATE as f64,
        }
    }

    pub fn decode_chunk<R: Read>(&mut self, reader: &mut R, mut on_mono: impl FnMut(f32)) -> std::io::Result<usize> {
        let mut raw = vec![0u8; 4096];
        let n = reader.read(&mut raw)?;
        if n == 0 {
            return Ok(0);
        }
        raw.truncate(n);
        let bps = self.format.bytes_per_sample();
        let channels = self.format.channels as usize;
        let frame_bytes = bps * channels;
        let mut frames = 0usize;

        for chunk in raw.chunks(frame_bytes) {
            if chunk.len() < frame_bytes {
                break;
            }
            let mono = self.frame_to_mono(chunk);
            self.emit_resampled(mono, &mut on_mono);
            frames += 1;
        }
        Ok(frames)
    }

    fn frame_to_mono(&self, frame: &[u8]) -> f32 {
        let bps = self.format.bytes_per_sample();
        match (self.format.channels, bps) {
            (1, 2) => i16::from_le_bytes([frame[0], frame[1]]) as f32,
            (2, 2) => {
                let l = i16::from_le_bytes([frame[0], frame[1]]) as f32;
                let r = i16::from_le_bytes([frame[2], frame[3]]) as f32;
                (l + r) * 0.5
            }
            (2, 4) => {
                let l = i32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as f32;
                let r = i32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]) as f32;
                (l + r) * 0.5 / 65536.0 * 32768.0
            }
            _ => 0.0,
        }
    }

    fn emit_resampled(&mut self, sample: f32, on_mono: &mut impl FnMut(f32)) {
        self.resample_phase += 1.0;
        while self.resample_phase >= self.resample_ratio {
            self.resample_phase -= self.resample_ratio;
            on_mono(sample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn stereo_downmix_and_decimate() {
        let mut dec = PcmDecoder::new(44100, 2, PcmFormat {
            rate: 44100,
            channels: 2,
            format: "s16le".into(),
        });
        let mut out = Vec::new();
        // 44100 Hz stereo -> ~22050 mono
        let mut pcm = Vec::new();
        for _ in 0..4410 {
            pcm.extend_from_slice(&1000i16.to_le_bytes());
            pcm.extend_from_slice(&1000i16.to_le_bytes());
        }
        let mut cur = Cursor::new(pcm);
        loop {
            let n = dec.decode_chunk(&mut cur, |s| out.push(s)).unwrap();
            if n == 0 {
                break;
            }
        }
        assert!(out.len() > 2000 && out.len() < 2300);
    }
}
