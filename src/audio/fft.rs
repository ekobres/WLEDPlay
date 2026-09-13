use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;

pub const SAMPLES_FFT: usize = 512;
pub const SAMPLES_FFT_2: usize = 256;
pub const SAMPLE_RATE: u32 = 22050;
pub const FFT_DOWNSCALE: f32 = 0.40;

pub struct FftEngine {
    window: Vec<f32>,
    scratch: Vec<Complex<f32>>,
    output: Vec<Complex<f32>>,
    magnitudes: Vec<f32>,
    r2c: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
}

impl FftEngine {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(SAMPLES_FFT);
        let mut window = vec![0.0; SAMPLES_FFT];
        for (i, w) in window.iter_mut().enumerate() {
            *w = blackman_harris(i, SAMPLES_FFT);
        }
        let scratch = r2c.make_scratch_vec();
        let output = r2c.make_output_vec();
        Self {
            window,
            scratch,
            output,
            magnitudes: vec![0.0; SAMPLES_FFT_2],
            r2c,
        }
    }

    pub fn analyze(&mut self, samples: &[f32; SAMPLES_FFT]) -> (f32, f32) {
        let mut input: Vec<f32> = samples.to_vec();
        let mean: f32 = input.iter().sum::<f32>() / SAMPLES_FFT as f32;
        for s in &mut input {
            *s -= mean;
        }
        for (i, s) in input.iter_mut().enumerate() {
            *s *= self.window[i];
        }

        self.r2c
            .process_with_scratch(&mut input, &mut self.output, &mut self.scratch)
            .expect("fft");

        self.magnitudes[0] = 0.0;
        let mut major_peak = 1.0f32;
        let mut magnitude = 0.001f32;
        let bin_hz = SAMPLE_RATE as f32 / SAMPLES_FFT as f32;

        for i in 1..SAMPLES_FFT_2 {
            let re = self.output[i].re;
            let im = self.output[i].im;
            let mag = (re * re + im * im).sqrt();
            self.magnitudes[i] = mag;
            if mag > magnitude {
                magnitude = mag;
                major_peak = i as f32 * bin_hz;
            }
        }

        (major_peak.clamp(1.0, 11025.0), magnitude)
    }

    pub fn magnitudes(&self) -> &[f32] {
        &self.magnitudes
    }
}

fn blackman_harris(n: usize, n_fft: usize) -> f32 {
    let a0 = 0.35875;
    let a1 = 0.48829;
    let a2 = 0.14128;
    let a3 = 0.01168;
    let x = 2.0 * std::f32::consts::PI * n as f32 / n_fft as f32;
    a0 - a1 * x.cos() + a2 * (2.0 * x).cos() - a3 * (3.0 * x).cos()
}

pub fn fft_add_avg(mags: &[f32], from: usize, to: usize) -> f32 {
    let mut result = 0.0f32;
    for i in from..=to.min(mags.len() - 1) {
        result += mags[i];
    }
    result *= 0.0625;
    result / (to - from + 1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn fft_add_avg_sums_bin_range() {
        let mags = [0.0f32; SAMPLES_FFT_2];
        let mut mags = mags;
        mags[5] = 16.0;
        mags[6] = 32.0;
        mags[7] = 48.0;
        // (16+32+48) * 0.0625 / 3 = 2.0
        assert_relative_eq!(fft_add_avg(&mags, 5, 7), 2.0, epsilon = 1e-5);
    }

    #[test]
    fn fft_detects_dominant_tone() {
        let mut engine = FftEngine::new();
        let mut samples = [0.0f32; SAMPLES_FFT];
        let hz = 440.0f32;
        for (i, s) in samples.iter_mut().enumerate() {
            let t = i as f32 / SAMPLE_RATE as f32;
            *s = (2.0 * std::f32::consts::PI * hz * t).sin();
        }
        let (peak, mag) = engine.analyze(&samples);
        assert!((peak - hz).abs() < 50.0, "peak {peak} not near {hz}");
        assert!(mag > 0.0);
    }

    #[test]
    fn silence_has_low_magnitude() {
        let mut engine = FftEngine::new();
        let samples = [0.0f32; SAMPLES_FFT];
        let (peak, mag) = engine.analyze(&samples);
        assert_eq!(peak, 1.0);
        assert_relative_eq!(mag, 0.001, epsilon = 1e-6);
    }
}
