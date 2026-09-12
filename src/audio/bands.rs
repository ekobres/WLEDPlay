use crate::audio::fft::{fft_add_avg, FFT_DOWNSCALE, SAMPLES_FFT_2};

pub const NUM_GEQ_CHANNELS: usize = 16;

pub const FFT_RESULT_PINK: [f32; 16] = [
    1.70, 1.71, 1.73, 1.78, 1.68, 1.56, 1.55, 1.63, 1.79, 1.62, 1.80, 2.06, 2.47, 3.35, 6.83,
    9.55,
];

pub fn map_bands(mags: &[f32], sample_avg: f32, fft_calc: &mut [f32; NUM_GEQ_CHANNELS]) {
    if sample_avg.abs() > 0.5 {
        fft_calc[0] = fft_add_avg(mags, 1, 2);
        fft_calc[1] = fft_add_avg(mags, 2, 3);
        fft_calc[2] = fft_add_avg(mags, 3, 5);
        fft_calc[3] = fft_add_avg(mags, 5, 7);
        fft_calc[4] = fft_add_avg(mags, 7, 10);
        fft_calc[5] = fft_add_avg(mags, 10, 13);
        fft_calc[6] = fft_add_avg(mags, 13, 19);
        fft_calc[7] = fft_add_avg(mags, 19, 26);
        fft_calc[8] = fft_add_avg(mags, 26, 33);
        fft_calc[9] = fft_add_avg(mags, 33, 44);
        fft_calc[10] = fft_add_avg(mags, 44, 56);
        fft_calc[11] = fft_add_avg(mags, 56, 70);
        fft_calc[12] = fft_add_avg(mags, 70, 86);
        fft_calc[13] = fft_add_avg(mags, 86, 104);
        fft_calc[14] = fft_add_avg(mags, 104, 165) * 0.88;
        fft_calc[15] = fft_add_avg(mags, 165, 215.min(SAMPLES_FFT_2 - 1)) * 0.70;
    } else {
        for c in fft_calc.iter_mut() {
            *c *= 0.85;
            if *c < 4.0 {
                *c = 0.0;
            }
        }
    }
}

pub fn post_process(
    fft_calc: &mut [f32; NUM_GEQ_CHANNELS],
    fft_avg: &mut [f32; NUM_GEQ_CHANNELS],
    fft_result: &mut [u8; NUM_GEQ_CHANNELS],
    noise_gate_open: bool,
    mult_agc: f32,
    use_agc: bool,
    sample_gain: u8,
    input_level: u8,
    limiter_on: bool,
    decay_time: u16,
) {
    for i in 0..NUM_GEQ_CHANNELS {
        if noise_gate_open {
            fft_calc[i] *= FFT_RESULT_PINK[i];
            fft_calc[i] *= FFT_DOWNSCALE;
            let gain = if use_agc {
                mult_agc
            } else {
                sample_gain as f32 / 40.0 * input_level as f32 / 128.0 + 1.0 / 16.0
            };
            fft_calc[i] *= gain;
            if fft_calc[i] < 0.0 {
                fft_calc[i] = 0.0;
            }
        }

        if fft_calc[i] > fft_avg[i] {
            fft_avg[i] = fft_calc[i] * 0.75 + 0.25 * fft_avg[i];
        } else if decay_time < 1000 {
            fft_avg[i] = fft_calc[i] * 0.22 + 0.78 * fft_avg[i];
        } else if decay_time < 2000 {
            fft_avg[i] = fft_calc[i] * 0.17 + 0.83 * fft_avg[i];
        } else if decay_time < 3000 {
            fft_avg[i] = fft_calc[i] * 0.14 + 0.86 * fft_avg[i];
        } else {
            fft_avg[i] = fft_calc[i] * 0.1 + 0.9 * fft_avg[i];
        }

        fft_calc[i] = fft_calc[i].clamp(0.0, 1023.0);
        fft_avg[i] = fft_avg[i].clamp(0.0, 1023.0);

        let mut current = if limiter_on { fft_avg[i] } else { fft_calc[i] };

        // FFTScalingMode 3: sqrt
        current *= 0.38;
        current -= 6.0;
        if current > 1.0 {
            current = current.sqrt();
        } else {
            current = 0.0;
        }
        current *= 0.85 + i as f32 / 4.5;
        current = mapf(current, 0.0, 16.0, 0.0, 255.0);

        let post_gain = input_level as f32 / 128.0;
        let post_gain = if post_gain < 1.0 {
            (post_gain - 1.0) * 0.8 + 1.0
        } else {
            post_gain
        };
        if use_agc {
            current *= post_gain;
        }

        fft_result[i] = current.clamp(0.0, 255.0) as u8;
    }
}

fn mapf(x: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> f32 {
    if (in_max - in_min).abs() < f32::EPSILON {
        return out_min;
    }
    (x - in_min) * (out_max - out_min) / (in_max - in_min) + out_min
}
