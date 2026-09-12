//! WLED Audio Sync V2 packet encoding (44 bytes, header "00002").

use std::io::{Cursor, Read, Write};

pub const UDP_SYNC_HEADER_V2: [u8; 6] = *b"00002\0";
pub const PACKET_SIZE: usize = 44;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioSyncPacket {
    pub sample_raw: f32,
    pub sample_smth: f32,
    pub sample_peak: u8,
    pub fft_result: [u8; 16],
    pub fft_magnitude: f32,
    pub fft_major_peak: f32,
}

impl Default for AudioSyncPacket {
    fn default() -> Self {
        Self {
            sample_raw: 0.0,
            sample_smth: 0.0,
            sample_peak: 0,
            fft_result: [0; 16],
            fft_magnitude: 0.001,
            fft_major_peak: 1.0,
        }
    }
}

impl AudioSyncPacket {
    pub fn encode(&self) -> [u8; PACKET_SIZE] {
        let mut buf = [0u8; PACKET_SIZE];
        let mut c = Cursor::new(&mut buf[..]);
        c.write_all(&UDP_SYNC_HEADER_V2).unwrap();
        c.write_all(&[0u8; 2]).unwrap();
        write_f32(&mut c, self.sample_raw);
        write_f32(&mut c, self.sample_smth);
        c.write_all(&[self.sample_peak, 0]).unwrap();
        c.write_all(&self.fft_result).unwrap();
        c.write_all(&[0u8; 2]).unwrap();
        write_f32(&mut c, self.fft_magnitude);
        write_f32(&mut c, self.fft_major_peak.clamp(1.0, 11025.0));
        buf
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() != PACKET_SIZE {
            return Err(ProtocolError::InvalidSize(bytes.len()));
        }
        if bytes[0..6] != UDP_SYNC_HEADER_V2 {
            return Err(ProtocolError::InvalidHeader);
        }
        let mut c = Cursor::new(bytes);
        let mut header = [0u8; 6];
        c.read_exact(&mut header).unwrap();
        let mut reserved1 = [0u8; 2];
        c.read_exact(&mut reserved1).unwrap();
        let sample_raw = read_f32(&mut c);
        let sample_smth = read_f32(&mut c);
        let mut peak_reserved = [0u8; 2];
        c.read_exact(&mut peak_reserved).unwrap();
        let mut fft_result = [0u8; 16];
        c.read_exact(&mut fft_result).unwrap();
        let mut reserved3 = [0u8; 2];
        c.read_exact(&mut reserved3).unwrap();
        let fft_magnitude = read_f32(&mut c);
        let fft_major_peak = read_f32(&mut c);
        Ok(Self {
            sample_raw,
            sample_smth,
            sample_peak: peak_reserved[0],
            fft_result,
            fft_magnitude,
            fft_major_peak,
        })
    }

    pub fn silence() -> Self {
        Self::default()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("invalid packet size: {0}")]
    InvalidSize(usize),
    #[error("invalid sync header")]
    InvalidHeader,
}

fn write_f32<W: Write>(w: &mut W, v: f32) {
    w.write_all(&v.to_le_bytes()).unwrap();
}

fn read_f32<R: Read>(r: &mut R) -> f32 {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).unwrap();
    f32::from_le_bytes(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_v2_packet() {
        let pkt = AudioSyncPacket {
            sample_raw: 42.5,
            sample_smth: 38.0,
            sample_peak: 1,
            fft_result: std::array::from_fn(|i| (i as u8).wrapping_mul(10).wrapping_add(5)),
            fft_magnitude: 1234.5,
            fft_major_peak: 440.0,
        };
        let encoded = pkt.encode();
        assert_eq!(encoded.len(), 44);
        assert_eq!(encoded[0..6], UDP_SYNC_HEADER_V2);
        let decoded = AudioSyncPacket::decode(&encoded).unwrap();
        assert_eq!(decoded, pkt);
    }

    #[test]
    fn rejects_wrong_size() {
        assert!(AudioSyncPacket::decode(&[0u8; 43]).is_err());
    }
}
