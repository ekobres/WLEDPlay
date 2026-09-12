use wledplay::audio::analyzer::hz_to_bin;
use wledplay::audio::Analyzer;
use wledplay::config::AgcMode;
use wledplay::wled::protocol::AudioSyncPacket;

#[test]
fn silence_packet_is_44_bytes_with_v2_header() {
    let p = AudioSyncPacket::silence();
    let b = p.encode();
    assert_eq!(b.len(), 44);
    assert_eq!(&b[0..6], b"00002\0");
}

#[test]
fn sine_440_energy_near_expected_band() {
    let mut a = Analyzer::new(AgcMode::Off);
    let rate = 22050f32;
    for n in 0..22050 {
        let t = n as f32 / rate;
        let s = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 8000.0;
        a.ingest_mono_sample(s, n as u64 / 45);
    }
    let pkt = a.build_packet();
    let peak_bin = hz_to_bin(440.0);
    // band 6 covers ~560-818 Hz, band 7 covers ~818-1120 — 440 should land in bass/mid bands 4-6
    let bass_energy: u32 = pkt.fft_result[3..=6].iter().map(|&v| v as u32).sum();
    assert!(bass_energy > 100, "expected energy in mid bands, got {:?}", pkt.fft_result);
}
