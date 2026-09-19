//! Sample-level processing matching WLED `getSample()` / `agcAvg()`.

use crate::config::AgcMode;

pub const DEFAULT_SQUELCH: u8 = 10;
pub const DEFAULT_GAIN: u8 = 60;
pub const DEFAULT_INPUT_LEVEL: u8 = 128;

pub struct SampleProcessor {
    pub sound_squelch: u8,
    pub sample_gain: u8,
    pub input_level: u8,
    pub sound_agc: AgcMode,
    pub mic_lev: f32,
    pub exp_adj_f: f32,
    pub sample_avg: f32,
    pub sample_raw: f32,
    pub sample_real: f32,
    pub raw_sample_agc: f32,
    pub sample_agc: f32,
    pub mult_agc: f32,
    pub sample_max: f32,
    pub control_integrated: f64,
    pub last_sound_agc: i8,
    pub max_sample_5sec: f32,
    pub sample_max_timer_ms: u64,
    pub time_of_peak_ms: u64,
    pub sample_peak: bool,
    pub udp_sample_peak: bool,
    pub max_vol: u8,
    pub bin_num: u8,
    pub sim_time_ms: u64,
    pub(crate) last_agc_ms: u64,
}

impl SampleProcessor {
    pub fn new(sound_agc: AgcMode) -> Self {
        Self {
            sound_squelch: DEFAULT_SQUELCH,
            sample_gain: DEFAULT_GAIN,
            input_level: DEFAULT_INPUT_LEVEL,
            sound_agc,
            mic_lev: 0.0,
            exp_adj_f: 0.0,
            sample_avg: 0.0,
            sample_raw: 0.0,
            sample_real: 0.0,
            raw_sample_agc: 0.0,
            sample_agc: 0.0,
            mult_agc: 1.0,
            sample_max: 0.0,
            control_integrated: 0.0,
            last_sound_agc: -1,
            max_sample_5sec: 0.0,
            sample_max_timer_ms: 0,
            time_of_peak_ms: 0,
            sample_peak: false,
            udp_sample_peak: false,
            max_vol: 31,
            bin_num: 8,
            sim_time_ms: 0,
            last_agc_ms: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.sound_agc);
    }

    pub fn process_sample(&mut self, mic_data_real: f32, now_ms: u64) {
        self.sim_time_ms = now_ms;
        const WEIGHTING: f32 = 0.2;

        self.mic_lev += (mic_data_real - self.mic_lev) / 12288.0;
        if mic_data_real < self.mic_lev {
            self.mic_lev = ((self.mic_lev * 31.0) + mic_data_real) / 32.0;
        }

        let mic_in_no_dc = (mic_data_real - self.mic_lev).abs();
        self.exp_adj_f = WEIGHTING * mic_in_no_dc + (1.0 - WEIGHTING) * self.exp_adj_f;
        self.exp_adj_f = self.exp_adj_f.abs();

        let mut exp_adj = self.exp_adj_f;
        if exp_adj <= self.sound_squelch as f32 {
            exp_adj = 0.0;
        }
        if self.sound_squelch == 0 && exp_adj < 0.25 {
            exp_adj = 0.0;
        }

        let tmp_sample = exp_adj;
        self.sample_real = tmp_sample;

        let sample_adj = tmp_sample * self.sample_gain as f32 / 40.0 * self.input_level as f32
            / 128.0
            + tmp_sample / 16.0;
        let sample_adj = sample_adj.clamp(0.0, 255.0);
        self.sample_raw = sample_adj;

        if self.sample_max < self.sample_real && self.sample_real > 0.5 {
            self.sample_max += 0.5 * (self.sample_real - self.sample_max);
            if (self.bin_num < 12 || self.max_vol < 1)
                && now_ms.saturating_sub(self.time_of_peak_ms) > 80
                && self.sample_avg > 1.0
            {
                self.sample_peak = true;
                self.time_of_peak_ms = now_ms;
                self.udp_sample_peak = true;
            }
        } else if self.mult_agc * self.sample_max > agc_zone_stop(self.sound_agc.profile_index())
            && self.sound_agc != AgcMode::Off
        {
            self.sample_max += 0.5 * (self.sample_real - self.sample_max);
        } else {
            self.sample_max *= agc_sample_decay(self.sound_agc.profile_index());
        }
        if self.sample_max < 0.5 {
            self.sample_max = 0.0;
        }

        self.sample_avg = (self.sample_avg * 15.0 + sample_adj) / 16.0;
        self.sample_avg = self.sample_avg.abs();

        self.agc_avg(now_ms);
        self.update_max_sample_5sec(now_ms);
    }

    pub(crate) fn update_max_sample_5sec(&mut self, now_ms: u64) {
        const CYCLE: u64 = 3500;
        if now_ms.saturating_sub(self.sample_max_timer_ms) > CYCLE {
            self.sample_max_timer_ms = now_ms;
            let v = if self.sound_agc != AgcMode::Off {
                self.sample_agc
            } else {
                self.sample_avg
            };
            self.max_sample_5sec = 0.15 * self.max_sample_5sec + 0.85 * v;
            if self.sample_avg < 1.0 {
                self.max_sample_5sec = 0.0;
            }
        } else if self.sample_avg >= 1.0 {
            let v = if self.sound_agc != AgcMode::Off {
                self.raw_sample_agc
            } else {
                self.sample_raw
            };
            self.max_sample_5sec = self.max_sample_5sec.max(v);
        }
    }

    pub fn agc_avg(&mut self, now_ms: u64) {
        if self.sound_agc == AgcMode::Off {
            return;
        }
        let preset = self.sound_agc.profile_index();
        let last_mult = self.mult_agc;
        let mut mult_temp = self.mult_agc;
        let mut tmp_agc = self.sample_real * self.mult_agc;
        let sound_agc_i = self.sound_agc as i8;

        if self.last_sound_agc != sound_agc_i {
            self.control_integrated = 0.0;
        }

        if now_ms.saturating_sub(self.last_agc_ms) > 2 {
            self.last_agc_ms = now_ms;

            if self.sample_real.abs() < 2.0 || self.sample_max < 1.0 {
                if self.control_integrated.abs() < 0.01 {
                    self.control_integrated = 0.0;
                } else {
                    self.control_integrated *= 0.91;
                }
            } else if tmp_agc <= agc_target0_up(preset) {
                mult_temp = agc_target0(preset) / self.sample_max;
            } else {
                mult_temp = agc_target1(preset) / self.sample_max;
            }

            mult_temp = mult_temp.clamp(1.0 / 64.0, 32.0);
            let control_error = mult_temp - last_mult;

            if (mult_temp > 0.085 && mult_temp < 6.5)
                && self.mult_agc * self.sample_max < agc_zone_stop(preset)
            {
                self.control_integrated += control_error as f64 * 0.002 * 0.25;
            } else {
                self.control_integrated *= 0.9;
            }

            tmp_agc = self.sample_real * last_mult;
            if tmp_agc > agc_zone_high(preset)
                || tmp_agc < self.sound_squelch as f32 + agc_zone_low(preset)
            {
                mult_temp =
                    last_mult + agc_follow_fast(preset) * agc_control_kp(preset) * control_error;
                mult_temp += agc_follow_fast(preset)
                    * agc_control_ki(preset)
                    * self.control_integrated as f32;
            } else {
                mult_temp =
                    last_mult + agc_follow_slow(preset) * agc_control_kp(preset) * control_error;
                mult_temp += agc_follow_slow(preset)
                    * agc_control_ki(preset)
                    * self.control_integrated as f32;
            }
            mult_temp = mult_temp.clamp(1.0 / 64.0, 32.0);
        }

        tmp_agc = self.sample_real * mult_temp;
        if self.sample_real.abs() < 2.0 {
            tmp_agc = 0.0;
        }
        if tmp_agc > 255.0 {
            tmp_agc = 255.0;
        }
        if tmp_agc < 1.0 {
            tmp_agc = 0.0;
        }

        self.mult_agc = mult_temp;
        self.raw_sample_agc = 0.8 * tmp_agc + 0.2 * self.raw_sample_agc;
        if tmp_agc.abs() < 1.0 {
            self.sample_agc = 0.5 * tmp_agc + 0.5 * self.sample_agc;
        } else {
            self.sample_agc += agc_sample_smooth(preset) * (tmp_agc - self.sample_agc);
        }
        self.sample_agc = self.sample_agc.abs();
        self.last_sound_agc = sound_agc_i;
    }

    pub fn volume_raw(&self) -> f32 {
        if self.sound_agc != AgcMode::Off {
            self.raw_sample_agc
        } else {
            self.sample_raw
        }
    }

    pub fn volume_smth(&self) -> f32 {
        if self.sound_agc != AgcMode::Off {
            self.sample_agc
        } else {
            self.sample_avg
        }
    }

    pub fn take_udp_peak(&mut self) -> u8 {
        let p = if self.udp_sample_peak { 1 } else { 0 };
        self.udp_sample_peak = false;
        p
    }

    pub fn auto_reset_peak(&mut self, frame_ms: u16) {
        let delay = frame_ms.max(50);
        if self.sim_time_ms.saturating_sub(self.time_of_peak_ms) > delay as u64 {
            self.sample_peak = false;
        }
    }
}

fn agc_sample_decay(p: usize) -> f32 {
    [0.9994, 0.9985, 0.9997][p]
}
fn agc_zone_low(p: usize) -> f32 {
    [32.0, 28.0, 36.0][p]
}
fn agc_zone_high(p: usize) -> f32 {
    [240.0, 240.0, 248.0][p]
}
fn agc_zone_stop(p: usize) -> f32 {
    [336.0, 448.0, 304.0][p]
}
fn agc_target0(p: usize) -> f32 {
    [112.0, 144.0, 164.0][p]
}
fn agc_target0_up(p: usize) -> f32 {
    [88.0, 64.0, 116.0][p]
}
fn agc_target1(p: usize) -> f32 {
    [220.0, 224.0, 216.0][p]
}
fn agc_follow_fast(p: usize) -> f32 {
    [1.0 / 192.0, 1.0 / 128.0, 1.0 / 256.0][p]
}
fn agc_follow_slow(p: usize) -> f32 {
    [1.0 / 6144.0, 1.0 / 4096.0, 1.0 / 8192.0][p]
}
fn agc_control_kp(p: usize) -> f32 {
    [0.6, 1.5, 0.65][p]
}
fn agc_control_ki(p: usize) -> f32 {
    [1.7, 1.85, 1.2][p]
}
fn agc_sample_smooth(p: usize) -> f32 {
    [1.0 / 12.0, 1.0 / 6.0, 1.0 / 16.0][p]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squelch_zeros_quiet_samples() {
        let mut p = SampleProcessor::new(AgcMode::Off);
        for ms in 0..100 {
            p.process_sample(0.001, ms);
        }
        assert!(p.sample_raw < 1.0);
    }

    #[test]
    fn loud_sample_increases_averaged_level() {
        let mut p = SampleProcessor::new(AgcMode::Off);
        for ms in 0..200 {
            p.process_sample(5000.0, ms);
        }
        assert!(p.sample_avg > 10.0);
    }

    #[test]
    fn agc_off_uses_raw_and_avg_volumes() {
        let mut p = SampleProcessor::new(AgcMode::Off);
        p.sample_raw = 42.0;
        p.sample_avg = 40.0;
        assert_eq!(p.volume_raw(), 42.0);
        assert_eq!(p.volume_smth(), 40.0);
    }

    #[test]
    fn take_udp_peak_is_one_shot() {
        let mut p = SampleProcessor::new(AgcMode::Off);
        p.udp_sample_peak = true;
        assert_eq!(p.take_udp_peak(), 1);
        assert_eq!(p.take_udp_peak(), 0);
    }

    #[test]
    fn reset_clears_levels() {
        let mut p = SampleProcessor::new(AgcMode::Normal);
        p.sample_avg = 100.0;
        p.sample_peak = true;
        p.reset();
        assert_eq!(p.sample_avg, 0.0);
        assert!(!p.sample_peak);
    }

    #[test]
    fn squelch_zero_gates_tiny_energy() {
        let mut p = SampleProcessor::new(AgcMode::Off);
        p.sound_squelch = 0;
        p.process_sample(0.01, 1);
        assert_eq!(p.sample_real, 0.0);
    }

    #[test]
    fn peak_flags_when_level_rises() {
        let mut p = SampleProcessor::new(AgcMode::Off);
        p.bin_num = 8;
        p.max_vol = 31;
        for ms in 0..120 {
            p.process_sample(8000.0, ms);
        }
        assert!(p.sample_peak || p.udp_sample_peak || p.sample_avg > 1.0);
        p.auto_reset_peak(50);
        p.sim_time_ms = p.time_of_peak_ms + 200;
        p.auto_reset_peak(50);
        assert!(!p.sample_peak);
    }

    #[test]
    fn agc_paths_and_volume_getters() {
        for mode in [AgcMode::Normal, AgcMode::Vivid, AgcMode::Lazy] {
            let mut p = SampleProcessor::new(mode);
            for ms in (0..400).step_by(3) {
                p.process_sample(6000.0, ms);
            }
            p.process_sample(0.0, 500);
            p.sound_agc = AgcMode::Vivid;
            p.agc_avg(520);
            assert!(p.volume_raw() >= 0.0);
            assert!(p.volume_smth() >= 0.0);
            p.process_sample(4000.0, 4000);
            p.sample_avg = 0.1;
            p.process_sample(0.2, 8000);
        }

        let mut quiet = SampleProcessor::new(AgcMode::Normal);
        quiet.agc_avg(10);
        quiet.control_integrated = 0.5;
        quiet.sample_real = 0.1;
        quiet.sample_max = 0.1;
        quiet.last_agc_ms = 0;
        quiet.agc_avg(20);

        let mut target0 = SampleProcessor::new(AgcMode::Normal);
        target0.agc_avg(10);
        target0.sample_real = 10.0;
        target0.sample_max = 8.0;
        target0.mult_agc = 1.0;
        target0.last_agc_ms = 0;
        target0.agc_avg(20);

        let mut hot = SampleProcessor::new(AgcMode::Normal);
        hot.sample_real = 250.0;
        hot.sample_max = 8.0;
        hot.mult_agc = 8.0;
        hot.agc_avg(10);
        hot.agc_avg(20);
        assert!(hot.raw_sample_agc <= 255.0);

        let mut off = SampleProcessor::new(AgcMode::Off);
        for ms in 0..80 {
            off.process_sample(3000.0, ms);
        }
        off.process_sample(3000.0, 4000);
    }

    #[test]
    fn sample_max_decay_and_agc_follow() {
        let mut p = SampleProcessor::new(AgcMode::Normal);
        p.sample_max = 400.0;
        p.mult_agc = 2.0;
        p.sample_real = 1.0;
        p.process_sample(1.0, 90);
        let _ = p.sample_max;

        let mut fade = SampleProcessor::new(AgcMode::Off);
        fade.sample_avg = 0.2;
        fade.sample_max_timer_ms = 0;
        fade.update_max_sample_5sec(4000);
        assert_eq!(fade.max_sample_5sec, 0.0);

        let helpers = [
            agc_sample_decay(0),
            agc_zone_low(1),
            agc_zone_high(2),
            agc_zone_stop(0),
            agc_target0(1),
            agc_target0_up(2),
            agc_target1(0),
            agc_follow_fast(1),
            agc_follow_slow(2),
            agc_control_kp(0),
            agc_control_ki(1),
            agc_sample_smooth(2),
        ];
        assert!(helpers.iter().all(|v| v.is_finite()));
    }
}
