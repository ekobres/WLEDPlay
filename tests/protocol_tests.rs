use wledplay::wled::protocol::{AudioSyncPacket, PACKET_SIZE, UDP_SYNC_HEADER_V2};

#[test]
fn v2_packet_is_always_44_bytes() {
    let samples = [
        AudioSyncPacket::default(),
        AudioSyncPacket {
            sample_raw: 100.0,
            sample_smth: 50.0,
            sample_peak: 1,
            fft_result: [255; 16],
            fft_magnitude: 999.0,
            fft_major_peak: 880.0,
        },
    ];
    for pkt in samples {
        assert_eq!(pkt.encode().len(), PACKET_SIZE);
    }
}

#[test]
fn roundtrip_preserves_all_fields() {
    let original = AudioSyncPacket {
        sample_raw: 12.5,
        sample_smth: 11.0,
        sample_peak: 1,
        fft_result: std::array::from_fn(|i| i as u8 + 1),
        fft_magnitude: 42.0,
        fft_major_peak: 220.0,
    };
    let bytes = original.encode();
    assert_eq!(&bytes[0..6], UDP_SYNC_HEADER_V2);
    let decoded = AudioSyncPacket::decode(&bytes).unwrap();
    assert_eq!(decoded, original);
}
