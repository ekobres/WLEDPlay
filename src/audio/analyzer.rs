use crate::audio::bands::{map_bands, post_process, NUM_GEQ_CHANNELS};
use crate::audio::fft::{FftEngine, SAMPLES_FFT, SAMPLE_RATE};
use crate::audio::sample::SampleProcessor;
use crate::config::AgcMode;
use crate::wled::protocol::AudioSyncPacket;

pub struct Analyzer {
    pub samples: SampleProcessor,
    fft: FftEngine,
    pub(crate) sample_buffer: Vec<f32>,
    fft_calc: [f32; NUM_GEQ_CHANNELS],
    fft_avg: [f32; NUM_GEQ_CHANNELS],
    fft_result: [u8; NUM_GEQ_CHANNELS],
    fft_major_peak: f32,
    fft_magnitude: f32,
    my_magnitude: f32,
    /// Absolute count of analysis samples stored for departure windows.
    samples_seen: u64,
    /// Exclusive end index of the next window. Zero until the first slot.
    next_end: u64,
    stream_active: bool,
    limiter_on: bool,
    decay_time: u16,
    fft_frames: u64,
}

impl Analyzer {
    pub fn new(agc: AgcMode) -> Self {
        Self {
            samples: SampleProcessor::new(agc),
            fft: FftEngine::new(),
            sample_buffer: Vec::with_capacity(SAMPLES_FFT),
            fft_calc: [0.0; NUM_GEQ_CHANNELS],
            fft_avg: [0.0; NUM_GEQ_CHANNELS],
            fft_result: [0; NUM_GEQ_CHANNELS],
            fft_major_peak: 1.0,
            fft_magnitude: 0.001,
            my_magnitude: 0.001,
            samples_seen: 0,
            next_end: 0,
            stream_active: false,
            limiter_on: true,
            decay_time: 1400,
            fft_frames: 0,
        }
    }

    pub fn set_stream_active(&mut self, active: bool) {
        self.stream_active = active;
        if !active {
            self.reset_silence();
        }
    }

    pub fn set_agc(&mut self, agc: AgcMode) {
        self.samples.sound_agc = agc;
    }

    pub fn reset_silence(&mut self) {
        self.samples.reset();
        self.sample_buffer.clear();
        self.samples_seen = 0;
        self.next_end = 0;
        self.fft_calc.fill(0.0);
        self.fft_avg.fill(0.0);
        self.fft_result.fill(0);
        self.fft_major_peak = 1.0;
        self.fft_magnitude = 0.001;
        self.my_magnitude = 0.001;
    }

    pub fn ingest_mono_sample(&mut self, sample: f32, now_ms: u64) {
        self.samples.process_sample(sample, now_ms);
        self.sample_buffer.push(sample);
        self.samples_seen += 1;
        let cap = SAMPLE_RATE as usize;
        if self.sample_buffer.len() > cap {
            let drop = self.sample_buffer.len() - cap;
            self.sample_buffer.drain(0..drop);
        }
    }

    /// Samples between scheduled Audio Sync packets. At 50 Hz this is 441.
    pub fn departure_hop(update_hz: u32) -> usize {
        (SAMPLE_RATE / update_hz.max(1)).max(1) as usize
    }

    /// Render the window that ends on the next unsent slot.
    ///
    /// One call produces one spectrum. A later call without new audio returns
    /// `None`, so a departure cannot repeat the previous window. A stall that
    /// has more than four slots waiting skips to the newest one.
    pub fn take_departure(&mut self, hop: usize) -> Option<AudioSyncPacket> {
        let hop = hop.max(1) as u64;
        if self.next_end == 0 {
            self.next_end = SAMPLES_FFT as u64;
        }
        if self.samples_seen < self.next_end {
            return None;
        }
        let ready = 1 + (self.samples_seen - self.next_end) / hop;
        if ready > 4 {
            let extra = (self.samples_seen - self.next_end) / hop;
            self.next_end += extra * hop;
        }
        let end = self.next_end;
        let start = end - SAMPLES_FFT as u64;
        let buf_start = self.samples_seen - self.sample_buffer.len() as u64;
        let offset = (start - buf_start) as usize;
        let mut batch = [0.0f32; SAMPLES_FFT];
        batch.copy_from_slice(&self.sample_buffer[offset..offset + SAMPLES_FFT]);
        let end_ms = end.saturating_mul(1000) / u64::from(SAMPLE_RATE);
        self.finish_fft(&batch, end_ms);
        self.next_end = end + hop;
        let keep_from = self.next_end.saturating_sub(SAMPLES_FFT as u64);
        let buf_start = self.samples_seen - self.sample_buffer.len() as u64;
        if keep_from > buf_start {
            let drop = (keep_from - buf_start) as usize;
            let drop = drop.min(self.sample_buffer.len());
            self.sample_buffer.drain(0..drop);
        }
        Some(self.build_packet())
    }

    fn finish_fft(&mut self, batch: &[f32; SAMPLES_FFT], now_ms: u64) {
        self.samples.sim_time_ms = now_ms;
        let (peak, mag) = self.fft.analyze(batch);
        self.fft_major_peak = peak;
        self.fft_magnitude = mag;

        map_bands(
            self.fft.magnitudes(),
            self.samples.sample_avg,
            &mut self.fft_calc,
        );

        let gate = self.samples.sample_avg.abs() > 0.25;
        post_process(
            &mut self.fft_calc,
            &mut self.fft_avg,
            &mut self.fft_result,
            gate,
            self.samples.mult_agc,
            self.samples.sound_agc != AgcMode::Off,
            self.samples.sample_gain,
            self.samples.input_level,
            self.limiter_on,
            self.decay_time,
        );

        self.detect_fft_peak();
        self.my_magnitude = self.fft_magnitude;
        if self.samples.sound_agc != AgcMode::Off {
            self.my_magnitude *= self.samples.mult_agc;
        }
        if self.samples.volume_smth() < 1.0 {
            self.my_magnitude = 0.001;
        }
        self.fft_frames += 1;
    }

    pub(crate) fn detect_fft_peak(&mut self) {
        let mags = self.fft.magnitudes();
        let bin = self.samples.bin_num as usize;
        if self.samples.sample_avg > 1.0
            && self.samples.max_vol > 0
            && bin > 4
            && bin < mags.len()
            && mags[bin] > self.samples.max_vol as f32
            && self
                .samples
                .sim_time_ms
                .saturating_sub(self.samples.time_of_peak_ms)
                > 100
        {
            self.samples.sample_peak = true;
            self.samples.time_of_peak_ms = self.samples.sim_time_ms;
            self.samples.udp_sample_peak = true;
        }
    }

    pub fn build_packet(&mut self) -> AudioSyncPacket {
        self.samples.auto_reset_peak(50);
        let mut fft = [0u8; 16];
        for (i, v) in self.fft_result.iter().enumerate() {
            fft[i] = (*v).min(254);
        }
        AudioSyncPacket {
            sample_raw: self.samples.volume_raw(),
            sample_smth: self.samples.volume_smth(),
            sample_peak: self.samples.take_udp_peak(),
            fft_result: fft,
            fft_magnitude: self.my_magnitude,
            fft_major_peak: self.fft_major_peak,
        }
    }

    pub fn silence_packet(&self) -> AudioSyncPacket {
        AudioSyncPacket::silence()
    }

    pub fn fft_frames(&self) -> u64 {
        self.fft_frames
    }

    pub fn dominant_frequency(&self) -> f32 {
        self.fft_major_peak
    }

    pub fn rms_proxy(&self) -> f32 {
        self.samples.volume_smth() / 255.0
    }

    pub fn agc_gain(&self) -> f32 {
        self.samples.mult_agc
    }

    pub fn agc_enabled(&self) -> bool {
        self.samples.sound_agc != AgcMode::Off
    }

    pub fn fft_bands(&self) -> [u8; 16] {
        let mut fft = [0u8; 16];
        for (i, v) in self.fft_result.iter().enumerate() {
            fft[i] = (*v).min(254);
        }
        fft
    }
}

pub fn hz_to_bin(hz: f32) -> usize {
    ((hz / (SAMPLE_RATE as f32 / SAMPLES_FFT as f32)).round() as usize).min(255)
}

#[cfg(test)]
mod tests {
    use super::Analyzer;
    use crate::config::AgcMode;
    use crate::wled::protocol::AudioSyncPacket;

    #[test]
    fn silence_clears_stuck_peak_and_level() {
        let mut a = Analyzer::new(AgcMode::Normal);
        a.samples.sample_peak = true;
        a.samples.sample_agc = 168.0;
        a.samples.sample_avg = 168.0;
        a.set_stream_active(false);
        assert!(!a.samples.sample_peak);
        assert!(a.rms_proxy() < 0.01);
    }

    #[test]
    fn hz_to_bin_maps_frequency_to_fft_index() {
        assert_eq!(super::hz_to_bin(0.0), 0);
        let bin = super::hz_to_bin(440.0);
        assert!((5..=12).contains(&bin));
    }

    #[test]
    fn build_packet_caps_fft_bytes_at_254() {
        let mut a = Analyzer::new(AgcMode::Off);
        a.fft_result = [255; 16];
        let pkt = a.build_packet();
        assert!(pkt.fft_result.iter().all(|&b| b <= 254));
    }

    #[test]
    fn silence_packet_matches_default() {
        let a = Analyzer::new(AgcMode::Off);
        assert_eq!(a.silence_packet(), AudioSyncPacket::silence());
        assert!(!a.agc_enabled());
        assert!((a.agc_gain() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ingest_tone_runs_fft_and_getters() {
        let mut a = Analyzer::new(AgcMode::Normal);
        a.set_agc(AgcMode::Vivid);
        assert_eq!(a.samples.sound_agc, AgcMode::Vivid);
        a.set_agc(AgcMode::Normal);
        a.set_stream_active(true);
        for i in 0..900 {
            let t = i as f32 / 22050.0;
            let s = (t * 440.0 * 2.0 * std::f32::consts::PI).sin() * 14000.0;
            a.ingest_mono_sample(s, i as u64);
        }
        assert_eq!(Analyzer::departure_hop(50), 441);
        assert_eq!(Analyzer::departure_hop(0), 22050);
        assert!(a.take_departure(441).is_some());
        assert!(a.fft_frames() > 0);
        assert!(a.dominant_frequency() >= 1.0);
        let _ = a.fft_bands();
        let _ = a.rms_proxy();
        assert!(a.agc_enabled());
        assert!(a.agc_gain() > 0.0);
        let pkt = a.build_packet();
        assert!(pkt.fft_major_peak >= 1.0);

        a.samples.bin_num = super::hz_to_bin(440.0) as u8;
        a.samples.max_vol = 1;
        a.samples.sample_avg = 10.0;
        a.samples.time_of_peak_ms = 0;
        a.samples.sim_time_ms = 250;
        a.detect_fft_peak();

        a.samples.sample_avg = 0.4;
        a.samples.sample_agc = 0.2;
        a.samples.sound_agc = AgcMode::Normal;
        a.samples.mult_agc = 1.5;
        a.finish_fft(&[0.2; crate::audio::fft::SAMPLES_FFT], 10_000);
        a.set_stream_active(true);
    }

    fn push_level(a: &mut Analyzer, n: usize, level: f32) {
        let seen = a.samples_seen;
        for i in 0..n {
            a.ingest_mono_sample(level, seen + i as u64);
        }
    }

    #[test]
    fn each_departure_renders_once_and_a_burst_keeps_both_windows() {
        let mut a = Analyzer::new(AgcMode::Off);
        push_level(&mut a, 512, 0.0);
        let first = a.take_departure(441).expect("first window");
        assert!(a.take_departure(441).is_none());
        assert!(first.fft_result.iter().all(|&b| b == 0));

        let seen = a.samples_seen as usize;
        for i in 0..441 {
            let t = (seen + i) as f32 / 22050.0;
            let s = (t * 440.0 * 2.0 * std::f32::consts::PI).sin() * 14_000.0;
            a.ingest_mono_sample(s, (seen + i) as u64);
        }
        let second = a.take_departure(441).expect("second window");
        assert!(a.take_departure(441).is_none());
        assert!(second.fft_result.iter().any(|&b| b > 0));
        assert_ne!(first.fft_result, second.fft_result);
    }

    #[test]
    fn stalled_departures_keep_the_newest_window() {
        let mut a = Analyzer::new(AgcMode::Off);
        push_level(&mut a, 512 + 441 * 10, 1_000.0);
        assert!(a.take_departure(441).is_some());
        assert!(a.take_departure(441).is_none());
        assert_eq!(a.fft_frames(), 1);
    }

    #[test]
    fn sample_buffer_cap_still_renders_the_newest_window() {
        let mut a = Analyzer::new(AgcMode::Off);
        push_level(&mut a, crate::audio::fft::SAMPLE_RATE as usize + 8, 2_000.0);
        assert!(a.sample_buffer.len() <= crate::audio::fft::SAMPLE_RATE as usize);
        assert!(a.take_departure(441).is_some());
        assert!(a.take_departure(441).is_none());
    }
}
