use crate::audio::bands::{map_bands, post_process, NUM_GEQ_CHANNELS};
use crate::audio::fft::{FftEngine, SAMPLES_FFT, SAMPLE_RATE};
use crate::audio::sample::SampleProcessor;
use crate::config::AgcMode;
use crate::wled::protocol::AudioSyncPacket;

pub struct Analyzer {
    pub samples: SampleProcessor,
    fft: FftEngine,
    sample_buffer: Vec<f32>,
    fft_calc: [f32; NUM_GEQ_CHANNELS],
    fft_avg: [f32; NUM_GEQ_CHANNELS],
    fft_result: [u8; NUM_GEQ_CHANNELS],
    fft_major_peak: f32,
    fft_magnitude: f32,
    my_magnitude: f32,
    last_fft_ms: u64,
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
            last_fft_ms: 0,
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

    pub fn reset_silence(&mut self) {
        self.sample_buffer.clear();
        self.fft_calc.fill(0.0);
        self.fft_avg.fill(0.0);
        self.fft_result.fill(0);
        self.fft_major_peak = 1.0;
        self.fft_magnitude = 0.001;
        self.my_magnitude = 0.001;
    }

    pub fn ingest_mono_sample(&mut self, sample: f32, now_ms: u64) {
        self.samples.process_sample(sample, now_ms);
        if self.samples.sample_avg > 0.25 {
            self.sample_buffer.push(sample);
        }
        if self.sample_buffer.len() >= SAMPLES_FFT
            && now_ms.saturating_sub(self.last_fft_ms) >= 21
        {
            self.run_fft(now_ms);
        }
    }

    fn run_fft(&mut self, now_ms: u64) {
        self.last_fft_ms = now_ms;
        let mut batch = [0.0f32; SAMPLES_FFT];
        let start = self.sample_buffer.len() - SAMPLES_FFT;
        batch.copy_from_slice(&self.sample_buffer[start..]);
        self.sample_buffer.drain(0..start.min(self.sample_buffer.len()));

        let (peak, mag) = self.fft.analyze(&batch);
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

    fn detect_fft_peak(&mut self) {
        let mags = self.fft.magnitudes();
        let bin = self.samples.bin_num as usize;
        if self.samples.sample_avg > 1.0
            && self.samples.max_vol > 0
            && bin > 4
            && bin < mags.len()
            && mags[bin] > self.samples.max_vol as f32
            && self.samples.sim_time_ms.saturating_sub(self.samples.time_of_peak_ms) > 100
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
