use wledplay::audio::analyzer::hz_to_bin;
use wledplay::audio::Analyzer;
use wledplay::config::AgcMode;
use wledplay::wled::protocol::AudioSyncPacket;

fn feed_sine(analyzer: &mut Analyzer, hz: f32, seconds: f32) {
    let rate = 22050f32;
    let samples = (rate * seconds) as usize;
    for n in 0..samples {
        let t = n as f32 / rate;
        let s = (2.0 * std::f32::consts::PI * hz * t).sin() * 8000.0;
        analyzer.ingest_mono_sample(s, (n as u64) / 45);
    }
}

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
    feed_sine(&mut a, 440.0, 1.0);
    let pkt = a.build_packet();
    let peak_bin = hz_to_bin(440.0);
    assert!((5..=12).contains(&peak_bin));
    let bass_energy: u32 = pkt.fft_result[3..=6].iter().map(|&v| v as u32).sum();
    assert!(
        bass_energy > 100,
        "expected energy in mid bands, got {:?} (peak_bin={peak_bin})",
        pkt.fft_result
    );
}

#[test]
fn silence_after_tone_clears_reactive_levels() {
    let mut a = Analyzer::new(AgcMode::Off);
    a.set_stream_active(true);
    feed_sine(&mut a, 440.0, 0.5);
    assert!(a.rms_proxy() > 0.05);
    a.set_stream_active(false);
    assert!(a.rms_proxy() < 0.01);
    assert_eq!(a.build_packet(), AudioSyncPacket::silence());
}
