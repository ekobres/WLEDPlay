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

    pub fn decode_chunk(
        &mut self,
        reader: &mut dyn Read,
        on_mono: &mut dyn FnMut(f32),
    ) -> std::io::Result<usize> {
        let mut raw = vec![0u8; 4096];
        let n = reader.read(&mut raw)?;
        if n == 0 {
            return Ok(0);
        }
        Ok(self.decode_bytes(&raw[..n], on_mono))
    }

    pub fn decode_chunk_into(
        &mut self,
        reader: &mut dyn Read,
        out: &mut Vec<f32>,
    ) -> std::io::Result<usize> {
        self.decode_chunk(reader, &mut |s| out.push(s))
    }

    pub fn decode_bytes_into(&mut self, raw: &[u8], out: &mut Vec<f32>) -> usize {
        self.decode_bytes(raw, &mut |s| out.push(s))
    }

    pub fn decode_chunk_discard(&mut self, reader: &mut dyn Read) -> std::io::Result<usize> {
        let mut out = Vec::new();
        self.decode_chunk_into(reader, &mut out)
    }

    pub fn decode_bytes_discard(&mut self, raw: &[u8]) -> usize {
        let mut out = Vec::new();
        self.decode_bytes_into(raw, &mut out)
    }

    pub fn decode_bytes(&mut self, raw: &[u8], on_mono: &mut dyn FnMut(f32)) -> usize {
        if raw.is_empty() {
            return 0;
        }
        let bps = self.format.bytes_per_sample();
        let channels = self.format.channels as usize;
        let frame_bytes = bps * channels;
        let mut frames = 0usize;

        for chunk in raw.chunks(frame_bytes) {
            if chunk.len() < frame_bytes {
                break;
            }
            let mono = self.frame_to_mono(chunk);
            self.emit_resampled(mono, on_mono);
            frames += 1;
        }
        frames
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

    fn emit_resampled(&mut self, sample: f32, on_mono: &mut dyn FnMut(f32)) {
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
        let mut dec = PcmDecoder::new(
            44100,
            2,
            PcmFormat {
                rate: 44100,
                channels: 2,
                format: "s16le".into(),
            },
        );
        let mut out = Vec::new();
        // 44100 Hz stereo -> ~22050 mono
        let mut pcm = Vec::new();
        for _ in 0..4410 {
            pcm.extend_from_slice(&1000i16.to_le_bytes());
            pcm.extend_from_slice(&1000i16.to_le_bytes());
        }
        let mut cur = Cursor::new(pcm);
        loop {
            let n = dec.decode_chunk_into(&mut cur, &mut out).unwrap();
            if n == 0 {
                break;
            }
        }
        assert!(out.len() > 2000 && out.len() < 2300);
    }

    #[test]
    fn mono_s16le_decodes() {
        let mut dec = PcmDecoder::new(
            22050,
            1,
            PcmFormat {
                rate: 22050,
                channels: 1,
                format: "s16le".into(),
            },
        );
        let pcm = 500i16.to_le_bytes();
        let mut cur = Cursor::new(pcm.to_vec());
        let mut out = Vec::new();
        dec.decode_chunk_into(&mut cur, &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert!((out[0] - 500.0).abs() < 1.0);
    }

    #[test]
    fn ignores_incomplete_trailing_frame() {
        let mut dec = PcmDecoder::new(
            44100,
            2,
            PcmFormat {
                rate: 44100,
                channels: 2,
                format: "s16le".into(),
            },
        );
        let mut pcm = Vec::new();
        pcm.extend_from_slice(&1000i16.to_le_bytes());
        pcm.extend_from_slice(&1000i16.to_le_bytes());
        pcm.push(0xAB); // one byte short of next frame
        let mut cur = Cursor::new(pcm);
        let mut out = Vec::new();
        let frames = dec.decode_chunk_into(&mut cur, &mut out).expect("decode");
        assert!(frames == 1);
    }

    #[test]
    fn empty_chunk_and_unknown_layout_are_zero() {
        let mut dec = PcmDecoder::new(22050, 1, PcmFormat::default());
        let mut cur = Cursor::new(Vec::<u8>::new());
        let empty_chunk = dec.decode_chunk_discard(&mut cur).expect("empty chunk");
        assert!(empty_chunk == 0);
        assert!(dec.decode_bytes_discard(&[]) == 0);

        let mut odd = PcmDecoder::new(
            22050,
            6,
            PcmFormat {
                rate: 22050,
                channels: 6,
                format: "s16le".into(),
            },
        );
        let mut out = Vec::new();
        let frame = [0u8; 12];
        assert_eq!(odd.decode_bytes_into(&frame, &mut out), 1);
        assert_eq!(out, vec![0.0]);
    }

    #[test]
    fn decode_chunk_propagates_read_error() {
        struct Boom;
        impl Read for Boom {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("fail"))
            }
        }
        let mut dec = PcmDecoder::new(22050, 1, PcmFormat::default());
        assert!(dec.decode_chunk_discard(&mut Boom).is_err());
    }

    #[test]
    fn stereo_s32le_downmixes() {
        let mut dec = PcmDecoder::new(
            22050,
            2,
            PcmFormat {
                rate: 22050,
                channels: 2,
                format: "s32le".into(),
            },
        );
        let mut pcm = Vec::new();
        pcm.extend_from_slice(&65_536i32.to_le_bytes());
        pcm.extend_from_slice(&65_536i32.to_le_bytes());
        let mut out = Vec::new();
        dec.decode_bytes_into(&pcm, &mut out);
        assert_eq!(out.len(), 1);
        assert!((out[0] - 32768.0).abs() < 1.0);
    }
}
