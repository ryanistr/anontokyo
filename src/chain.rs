//! Effect chain: preamp -> boosts -> EQ (simple/multi) -> soft limiter.
//!
//! [`ChainDef`] is immutable once built and shared lock-free via arc-swap;
//! the per-channel filter states live in [`ChainStates`, owned by the
//! realtime audio thread].

use crate::biquad::{Biquad, BiquadState};
use crate::settings::{EqMode, Settings};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

const LIMITER_KNEE: f64 = 0.85;
/// slightly under full scale: f64 tanh saturates to exactly 1.0 for large
/// arguments, and 1.0f32 clips when PipeWire converts to integer samples
const LIMITER_CEILING: f64 = 0.999;

pub struct ChainDef {
    pub generation: u64,
    pub rate: u32,
    pub channels: usize,
    pub bypass: bool,
    pub preamp_db: f32,
    pub stages: Vec<Biquad>,
    pub limiter: bool,
}

impl ChainDef {
    pub fn new(settings: &Settings, rate: u32, channels: usize) -> Self {
        let sr = rate.max(8000) as f64;
        let mut stages = Vec::new();

        if !settings.bypass {
            let bass = &settings.bass;
            if bass.enabled && bass.gain_db.abs() > 0.01 {
                stages.push(Biquad::low_shelf(
                    sr,
                    bass.freq_hz as f64,
                    bass.gain_db as f64,
                ));
            }
            let vocal = &settings.vocal;
            if vocal.enabled && vocal.gain_db.abs() > 0.01 {
                stages.push(Biquad::peaking(
                    sr,
                    vocal.freq_hz as f64,
                    vocal.gain_db as f64,
                    vocal.q as f64,
                ));
            }
            let treble = &settings.treble;
            if treble.enabled && treble.gain_db.abs() > 0.01 {
                stages.push(Biquad::high_shelf(
                    sr,
                    treble.freq_hz as f64,
                    treble.gain_db as f64,
                ));
            }

            match settings.eq_mode {
                EqMode::Simple => {
                    if let [low, mid, high] = &settings.simple_bands[..] {
                        if low.gain_db.abs() > 0.01 {
                            stages.push(Biquad::low_shelf(
                                sr,
                                low.freq_hz as f64,
                                low.gain_db as f64,
                            ));
                        }
                        if mid.gain_db.abs() > 0.01 {
                            stages.push(Biquad::peaking(
                                sr,
                                mid.freq_hz as f64,
                                mid.gain_db as f64,
                                mid.q as f64,
                            ));
                        }
                        if high.gain_db.abs() > 0.01 {
                            stages.push(Biquad::high_shelf(
                                sr,
                                high.freq_hz as f64,
                                high.gain_db as f64,
                            ));
                        }
                    }
                }
                EqMode::Multi => {
                    for band in &settings.multi_bands {
                        if band.gain_db.abs() > 0.01 {
                            stages.push(Biquad::peaking(
                                sr,
                                band.freq_hz as f64,
                                band.gain_db as f64,
                                band.q as f64,
                            ));
                        }
                    }
                }
            }
        }

        Self {
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            rate,
            channels,
            bypass: settings.bypass,
            preamp_db: settings.preamp_db,
            stages,
            limiter: settings.limiter && !settings.bypass,
        }
    }

    /// Small-signal magnitude in dB (preamp + all stages; limiter/bypass excluded).
    pub fn magnitude_db(&self, freq: f64) -> f64 {
        if self.bypass {
            return 0.0;
        }
        let mut db = self.preamp_db as f64;
        for stage in &self.stages {
            db += stage.magnitude_db(freq, self.rate as f64);
        }
        db
    }

    /// Process interleaved f32 samples in place.
    pub fn process(&self, states: &mut ChainStates, buf: &mut [f32]) {
        if self.bypass || self.stages.is_empty() {
            if self.limiter {
                for s in buf.iter_mut() {
                    *s = soft_limit(*s as f64) as f32;
                }
            }
            if !self.bypass && self.preamp_db.abs() > 0.01 {
                let g = 10f64.powf(self.preamp_db as f64 / 20.0) as f32;
                for s in buf.iter_mut() {
                    *s *= g;
                }
            }
            return;
        }
        states.sync(self.generation, self.stages.len(), self.channels);
        let preamp = 10f64.powf(self.preamp_db as f64 / 20.0);
        let ch = self.channels.max(1);
        let frames = buf.len() / ch;
        for f in 0..frames {
            for c in 0..ch {
                let idx = f * ch + c;
                let mut x = (buf[idx] as f64) * preamp;
                let chan_states = &mut states.states[c];
                for (stage, state) in self.stages.iter().zip(chan_states.iter_mut()) {
                    x = stage.process_sample(state, x);
                }
                if self.limiter {
                    x = soft_limit(x);
                }
                buf[idx] = x as f32;
            }
        }
        // trailing partial frame (shouldn't happen with even channel counts)
        for s in &mut buf[frames * ch..] {
            if self.limiter {
                *s = soft_limit(*s as f64) as f32;
            }
        }
    }
}

#[inline]
fn soft_limit(x: f64) -> f64 {
    let a = x.abs();
    if a <= LIMITER_KNEE {
        x
    } else {
        let over = (a - LIMITER_KNEE) / (1.0 - LIMITER_KNEE);
        (LIMITER_KNEE + (1.0 - LIMITER_KNEE) * over.tanh())
            .min(LIMITER_CEILING)
            .copysign(x)
    }
}

pub struct ChainStates {
    generation: u64,
    pub states: Vec<Vec<BiquadState>>,
}

impl Default for ChainStates {
    fn default() -> Self {
        Self {
            generation: 0,
            states: Vec::new(),
        }
    }
}

impl ChainStates {
    /// Reallocate filter states when the chain definition changed.
    /// Only ever called from the realtime thread, only allocates when the
    /// user actually changed a setting (rare, user-triggered).
    fn sync(&mut self, generation: u64, stages: usize, channels: usize) {
        if self.generation != generation {
            self.states = vec![vec![BiquadState::default(); stages]; channels.max(1)];
            self.generation = generation;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Band;

    const SR: u32 = 48_000;

    fn sine(buf: &mut [f32], hz: f64, rate: f64) {
        for (i, s) in buf.iter_mut().enumerate() {
            *s = (2.0 * std::f64::consts::PI * hz * i as f64 / rate).sin() as f32;
        }
    }

    fn rms(buf: &[f32]) -> f64 {
        let sum: f64 = buf.iter().map(|&s| (s as f64) * (s as f64)).sum();
        (sum / buf.len() as f64).sqrt()
    }

    fn steady_rms(buf: &[f32]) -> f64 {
        // skip filter transient: first quarter
        rms(&buf[buf.len() / 4..])
    }

    #[test]
    fn bypass_is_bit_exact_passthrough() {
        let settings = Settings {
            bypass: true,
            ..Settings::default()
        };
        let def = ChainDef::new(&settings, SR, 2);
        let mut states = ChainStates::default();
        let mut buf = vec![0.0f32; 4096];
        sine(&mut buf, 440.0, SR as f64);
        let original = buf.clone();
        def.process(&mut states, &mut buf);
        assert_eq!(buf, original);
    }

    #[test]
    fn flat_settings_do_not_change_signal() {
        let settings = Settings::default(); // everything off/zero
        let def = ChainDef::new(&settings, SR, 2);
        let mut states = ChainStates::default();
        let mut buf = vec![0.0f32; 8192];
        sine(&mut buf, 440.0, SR as f64);
        for s in &mut buf {
            *s *= 0.5; // below the limiter knee
        }
        let original = buf.clone();
        def.process(&mut states, &mut buf);
        let max_diff = buf
            .iter()
            .zip(&original)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_diff < 1e-6, "max diff {max_diff}");
    }

    #[test]
    fn bass_boost_raises_low_frequency_energy() {
        let mut settings = Settings::default();
        settings.bass.enabled = true;
        settings.bass.gain_db = 10.0;
        settings.bass.freq_hz = 150.0;
        let def = ChainDef::new(&settings, SR, 1);

        let mut flat = vec![0.0f32; SR as usize]; // 1 second
        sine(&mut flat, 60.0, SR as f64);
        for s in &mut flat {
            *s *= 0.1; // stay under the limiter knee: measure the filter only
        }
        let mut processed = flat.clone();
        let mut states = ChainStates::default();
        def.process(&mut states, &mut processed);

        let delta_db = 20.0 * (steady_rms(&processed) / steady_rms(&flat)).log10();
        let predicted = def.magnitude_db(60.0);
        assert!(
            (delta_db - predicted).abs() < 0.3,
            "measured {delta_db}, predicted {predicted}"
        );
        assert!(
            predicted > 8.0,
            "expected strong bass gain, got {predicted}"
        );
    }

    #[test]
    fn vocal_boost_raises_presence_band() {
        let mut settings = Settings::default();
        settings.vocal.enabled = true;
        settings.vocal.gain_db = 8.0;
        settings.vocal.freq_hz = 3000.0;
        let def = ChainDef::new(&settings, SR, 1);
        assert!((def.magnitude_db(3000.0) - 8.0).abs() < 0.2);

        let mut buf = vec![0.0f32; SR as usize];
        sine(&mut buf, 3000.0, SR as f64);
        for s in &mut buf {
            *s *= 0.1; // stay under the limiter knee: measure the filter only
        }
        let original = buf.clone();
        let mut states = ChainStates::default();
        def.process(&mut states, &mut buf);
        let delta_db = 20.0 * (steady_rms(&buf) / steady_rms(&original)).log10();
        assert!((delta_db - 8.0).abs() < 0.3, "measured {delta_db}");
    }

    #[test]
    fn boosts_and_eq_stack_additively() {
        let mut settings = Settings::default();
        settings.bass.enabled = true;
        settings.bass.gain_db = 6.0;
        settings.bass.freq_hz = 150.0;
        settings.eq_mode = EqMode::Simple;
        settings.simple_bands[0] = Band::new(150.0, 4.0, 0.7); // same shelf again
        let def = ChainDef::new(&settings, SR, 1);
        // two stacked shelves at the same freq => gains add at DC-ish
        let total = def.magnitude_db(30.0);
        assert!((total - 10.0).abs() < 0.3, "stacked gain {total}");
    }

    #[test]
    fn multi_mode_uses_only_multi_bands() {
        let mut settings = Settings::default();
        settings.eq_mode = EqMode::Multi;
        settings.simple_bands[1] = Band::new(1000.0, 15.0, 1.0); // ignored in multi mode
        settings.multi_bands[0] = Band::new(63.0, 6.0, 1.4);
        let def = ChainDef::new(&settings, SR, 1);
        assert!(def.magnitude_db(1000.0).abs() < 0.2, "mid must stay flat");
        assert!((def.magnitude_db(63.0) - 6.0).abs() < 0.3);
    }

    #[test]
    fn limiter_caps_hot_output() {
        let mut settings = Settings::default();
        settings.limiter = true;
        settings.preamp_db = 12.0;
        settings.bass.enabled = true;
        settings.bass.gain_db = 12.0;
        let def = ChainDef::new(&settings, SR, 1);
        let mut states = ChainStates::default();
        let mut buf = vec![0.95f32; SR as usize / 4];
        def.process(&mut states, &mut buf);
        let peak = buf.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak < 1.0, "peak {peak} must stay under full scale");
    }

    #[test]
    fn limiter_leaves_quiet_signal_untouched() {
        let mut settings = Settings::default();
        settings.limiter = true;
        let def = ChainDef::new(&settings, SR, 1);
        let mut states = ChainStates::default();
        let mut buf = vec![0.0f32; 1024];
        sine(&mut buf, 1000.0, SR as f64);
        for s in &mut buf {
            *s *= 0.5;
        }
        let original = buf.clone();
        def.process(&mut states, &mut buf);
        let max_diff = buf
            .iter()
            .zip(&original)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_diff < 1e-7, "diff {max_diff}");
    }

    #[test]
    fn state_resyncs_after_regeneration() {
        let mut settings = Settings::default();
        settings.bass.enabled = true;
        let def1 = ChainDef::new(&settings, SR, 2);
        let mut states = ChainStates::default();
        let mut buf = vec![0.1f32; 512];
        def1.process(&mut states, &mut buf);

        settings.bass.gain_db = 3.0;
        let def2 = ChainDef::new(&settings, SR, 2);
        assert_ne!(def1.generation, def2.generation);
        let mut buf2 = vec![0.1f32; 512];
        def2.process(&mut states, &mut buf2); // must not panic / misindex
        assert_eq!(states.states.len(), 2);
        assert_eq!(states.states[0].len(), def2.stages.len());
    }
}
