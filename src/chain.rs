//! Effect chain: preamp -> [clarity, bass-boost slider, boosts] -> EQ -> dynamic boost
//! -> surround -> ambience -> soft limiter.
//!
//! [`ChainDef`] is immutable once built and shared lock-free via arc-swap;
//! the per-channel filter states live in [`ChainStates`, owned by the
//! realtime audio thread].
//!
//! The FxSound-style stages (dynamic boost / surround / ambience) are
//! level-dependent, so `magnitude_db` only models the biquad part of the chain.

use crate::biquad::{Biquad, BiquadState};
use crate::settings::{EqMode, Settings};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

const LIMITER_KNEE: f64 = 0.85;
/// slightly under full scale: f64 tanh saturates to exactly 1.0 for large
/// arguments, and 1.0f32 clips when PipeWire converts to integer samples
const LIMITER_CEILING: f64 = 0.999;

/// Dynamic boost: upward compressor lifting anything below `target_db`
/// by up to `max_db` (slider 0..100 -> 0..12 dB).
pub struct DynDef {
    pub max_db: f64,
    pub target_db: f64,
    pub k_att: f64,
    pub k_rel: f64,
}

/// Multitap early-reflection pattern: per-channel tap offsets in samples
/// (right channel slightly stretched for decorrelation) and a shared ring.
pub struct AmbienceDef {
    pub wet: f64,
    pub ring_len: usize,
    pub offsets: Vec<Vec<(usize, f64)>>,
}

const DYN_TARGET_DB: f64 = -26.0;
const AMBIENCE_TAPS: [(f64, f64); 4] = [(0.017, 1.0), (0.029, 0.8), (0.041, 0.62), (0.053, 0.47)];
const AMBIENCE_WET_MAX: f64 = 0.35;
/// ring covers every tap including the right-channel stretch
const AMBIENCE_RING_S: f64 = 0.060;

fn ambience_def(amount: f32, sr: f64, channels: usize) -> AmbienceDef {
    let wet = amount as f64 / 100.0 * AMBIENCE_WET_MAX;
    let ring_len = (AMBIENCE_RING_S * sr).ceil() as usize + 2;
    let offsets = (0..channels.max(1))
        .map(|c| {
            let stretch = if c % 2 == 1 { 1.06 } else { 1.0 };
            AMBIENCE_TAPS
                .iter()
                .map(|(s, g)| {
                    (
                        ((s * stretch * sr).round() as usize).clamp(1, ring_len - 1),
                        *g,
                    )
                })
                .collect()
        })
        .collect();
    AmbienceDef {
        wet,
        ring_len,
        offsets,
    }
}

pub struct ChainDef {
    pub generation: u64,
    pub rate: u32,
    pub channels: usize,
    pub bypass: bool,
    pub preamp_db: f32,
    pub stages: Vec<Biquad>,
    pub limiter: bool,
    pub dyn_boost: Option<DynDef>,
    /// M/S width factor; 1.0 = off (stateless, applied to the first stereo pair)
    pub surround_width: f64,
    pub ambience: Option<AmbienceDef>,
}

impl ChainDef {
    pub fn new(settings: &Settings, rate: u32, channels: usize) -> Self {
        let sr = rate.max(8000) as f64;
        let mut stages = Vec::new();
        let mut dyn_boost = None;
        let mut surround_width = 1.0;
        let mut ambience = None;

        if !settings.bypass {
            // FxSound-style sliders stack in front of the boost toggles
            let clarity = &settings.clarity;
            if clarity.enabled && clarity.amount > 0.01 {
                stages.push(Biquad::peaking(
                    sr,
                    4000.0,
                    clarity.amount as f64 * 0.09,
                    0.9,
                ));
            }
            let bass_boost = &settings.bass_boost;
            if bass_boost.enabled && bass_boost.amount > 0.01 {
                stages.push(Biquad::low_shelf(
                    sr,
                    100.0,
                    bass_boost.amount as f64 * 0.12,
                ));
            }

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

            // per-frame fx, running after the biquad stack
            let dyn_fx = &settings.dynamic_boost;
            if dyn_fx.enabled && dyn_fx.amount > 0.01 {
                dyn_boost = Some(DynDef {
                    max_db: dyn_fx.amount as f64 * 0.12,
                    target_db: DYN_TARGET_DB,
                    k_att: (-1.0 / (0.005 * sr)).exp(),
                    k_rel: (-1.0 / (0.150 * sr)).exp(),
                });
            }
            if settings.surround.enabled && settings.surround.amount > 0.01 {
                surround_width = 1.0 + settings.surround.amount as f64 / 100.0;
            }
            let amb_fx = &settings.ambience;
            if amb_fx.enabled && amb_fx.amount > 0.01 {
                ambience = Some(ambience_def(amb_fx.amount, sr, channels));
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
            dyn_boost,
            surround_width,
            ambience,
        }
    }

    /// Small-signal magnitude in dB (preamp + biquad stages; bypass, limiter
    /// and the level-dependent fx stages excluded).
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
        let has_fx =
            self.dyn_boost.is_some() || self.surround_width > 1.0 || self.ambience.is_some();
        if self.bypass || (self.stages.is_empty() && !has_fx) {
            if !self.bypass && self.preamp_db.abs() > 0.01 {
                let g = 10f64.powf(self.preamp_db as f64 / 20.0) as f32;
                for s in buf.iter_mut() {
                    *s *= g;
                }
            }
            if self.limiter {
                for s in buf.iter_mut() {
                    *s = soft_limit(*s as f64) as f32;
                }
            }
            return;
        }
        states.sync(self);
        let preamp = 10f64.powf(self.preamp_db as f64 / 20.0);
        let ch = self.channels.max(1);
        let frames = buf.len() / ch;
        for f in 0..frames {
            for c in 0..ch {
                let idx = f * ch + c;
                let mut x = (buf[idx] as f64) * preamp;
                if !self.stages.is_empty() {
                    let chan_states = &mut states.states[c];
                    for (stage, state) in self.stages.iter().zip(chan_states.iter_mut()) {
                        x = stage.process_sample(state, x);
                    }
                }
                if let Some(dbt) = &self.dyn_boost {
                    x = dynamic_lift(x, &mut states.dyn_env[c], dbt);
                }
                buf[idx] = x as f32;
            }
            if self.surround_width > 1.0 && ch == 2 {
                let (l, r) = (buf[f * ch] as f64, buf[f * ch + 1] as f64);
                let m = (l + r) * 0.5;
                let side = (l - r) * 0.5 * self.surround_width;
                buf[f * ch] = (m + side) as f32;
                buf[f * ch + 1] = (m - side) as f32;
            }
            if let Some(amb) = &self.ambience {
                for c in 0..amb.offsets.len().min(ch) {
                    let idx = f * ch + c;
                    let x = buf[idx] as f64;
                    let head = states.amb_head[c];
                    let ring = &mut states.amb_ring[c];
                    ring[head] = x;
                    let mut wet = 0.0;
                    for &(off, g) in &amb.offsets[c] {
                        wet += g * ring[(head + amb.ring_len - off) % amb.ring_len];
                    }
                    states.amb_head[c] = (head + 1) % amb.ring_len;
                    buf[idx] = (x + amb.wet * wet) as f32;
                }
            }
        }
        if self.limiter {
            for s in buf.iter_mut() {
                *s = soft_limit(*s as f64) as f32;
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

/// Envelope-following upward lift; `env` is the per-channel peak envelope.
#[inline]
fn dynamic_lift(x: f64, env: &mut f64, dyn_def: &DynDef) -> f64 {
    let a = x.abs();
    let k = if a > *env {
        dyn_def.k_att
    } else {
        dyn_def.k_rel
    };
    *env = a * k + *env * (1.0 - k);
    let level_db = 20.0 * (*env + 1e-9).log10();
    let gain_db = (dyn_def.target_db - level_db).max(0.0).min(dyn_def.max_db);
    x * 10f64.powf(gain_db / 20.0)
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
    dyn_env: Vec<f64>,
    amb_ring: Vec<Vec<f64>>,
    amb_head: Vec<usize>,
}

impl Default for ChainStates {
    fn default() -> Self {
        Self {
            generation: 0,
            states: Vec::new(),
            dyn_env: Vec::new(),
            amb_ring: Vec::new(),
            amb_head: Vec::new(),
        }
    }
}

impl ChainStates {
    /// Reallocate filter/fx states when the chain definition changed.
    /// Only ever called from the realtime thread, only allocates when the
    /// user actually changed a setting (rare, user-triggered).
    fn sync(&mut self, def: &ChainDef) {
        if self.generation == def.generation {
            return;
        }
        self.generation = def.generation;
        let ch = def.channels.max(1);
        self.states = vec![vec![BiquadState::default(); def.stages.len()]; ch];
        self.dyn_env = vec![0.0; ch];
        match &def.ambience {
            Some(amb) => {
                self.amb_ring = (0..ch).map(|_| vec![0.0; amb.ring_len]).collect();
                self.amb_head = vec![0; ch];
            }
            None => {
                self.amb_ring.clear();
                self.amb_head.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Band, Fx};

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

    // ---------- FxSound-style slider effects ----------

    #[test]
    fn clarity_slider_lifts_presence() {
        let mut settings = Settings::default();
        settings.clarity = Fx::new(true, 100.0); // +9 dB peaking @4kHz
        let def = ChainDef::new(&settings, SR, 1);
        let gain = def.magnitude_db(4000.0);
        assert!((gain - 9.0).abs() < 0.3, "clarity@100 => 9dB, got {gain}");

        settings.clarity.enabled = false;
        let flat = ChainDef::new(&settings, SR, 1);
        assert!(flat.magnitude_db(4000.0).abs() < 1e-9);
    }

    #[test]
    fn bass_boost_slider_shelves_lows_independently_of_toggle() {
        let mut settings = Settings::default();
        settings.bass_boost = Fx::new(true, 50.0); // 6 dB shelf @100Hz
        let slider_only = ChainDef::new(&settings, SR, 1);
        let at_corner = slider_only.magnitude_db(100.0);
        assert!(
            (at_corner - 3.0).abs() < 0.5,
            "half gain at corner, got {at_corner}"
        );
        assert!(slider_only.magnitude_db(30.0) > 5.0);

        settings.bass.enabled = true;
        settings.bass.gain_db = 10.0;
        settings.bass.freq_hz = 150.0;
        let toggle_only = {
            let mut t = Settings::default();
            t.bass.enabled = true;
            t.bass.gain_db = 10.0;
            t.bass.freq_hz = 150.0;
            ChainDef::new(&t, SR, 1)
        };
        let both = ChainDef::new(&settings, SR, 1);
        // LTI biquads in series: dB magnitudes sum exactly
        let summed = toggle_only.magnitude_db(30.0) + slider_only.magnitude_db(30.0);
        assert!((both.magnitude_db(30.0) - summed).abs() < 0.1);
    }

    #[test]
    fn dynamic_boost_lifts_quiet_and_passes_loud() {
        let mut settings = Settings::default();
        settings.dynamic_boost = Fx::new(true, 100.0); // +12 dB max
        settings.limiter = false; // measure the stage itself
        let def = ChainDef::new(&settings, SR, 1);

        let mut quiet = vec![0.0f32; SR as usize];
        sine(&mut quiet, 200.0, SR as f64);
        for s in &mut quiet {
            *s *= 0.01; // -40 dBFS: 14 dB below target => capped at +12
        }
        let mut states = ChainStates::default();
        def.process(&mut states, &mut quiet);
        let out_peak = quiet[SR as usize / 2..]
            .iter()
            .map(|s| (*s as f64).abs())
            .fold(0.0f64, f64::max);
        let boost_db = 20.0 * (out_peak / 0.01).log10();
        assert!((boost_db - 12.0).abs() < 0.5, "quiet lifted {boost_db}dB");

        let mut loud = vec![0.0f32; SR as usize];
        sine(&mut loud, 200.0, SR as f64);
        for s in &mut loud {
            *s *= 0.5;
        }
        let mut states = ChainStates::default();
        def.process(&mut states, &mut loud);
        let out_peak = loud[SR as usize / 2..]
            .iter()
            .map(|s| (*s as f64).abs())
            .fold(0.0f64, f64::max);
        assert!(
            (out_peak - 0.5).abs() < 0.01,
            "loud must pass, got {out_peak}"
        );
    }

    #[test]
    fn surround_widens_side_signal_only() {
        let mut settings = Settings::default();
        settings.surround = Fx::new(true, 100.0); // width x2
        let def = ChainDef::new(&settings, SR, 2);

        let mut buf = vec![0.1f32; 32];
        for frame in buf.chunks_exact_mut(2) {
            frame[1] = -0.1; // pure side content
        }
        def.process(&mut ChainStates::default(), &mut buf);
        assert!((buf[0] - 0.2).abs() < 1e-6, "side widened to {}", buf[0]);
        assert!((buf[1] + 0.2).abs() < 1e-6);

        let mut mono = vec![0.1f32; 32]; // identical channels: nothing to widen
        def.process(&mut ChainStates::default(), &mut mono);
        assert!((mono[0] - 0.1).abs() < 1e-6, "mono must stay put");
    }

    #[test]
    fn ambience_places_reflection_taps() {
        let mut settings = Settings::default();
        settings.ambience = Fx::new(true, 50.0); // wet 0.175
        settings.limiter = false; // measure the stage itself
        let def = ChainDef::new(&settings, SR, 1);

        let mut buf = vec![0.0f32; SR as usize];
        buf[0] = 1.0; // impulse
        def.process(&mut ChainStates::default(), &mut buf);
        assert!((buf[0] - 1.0).abs() < 1e-6, "dry stays untouched");
        let tap = (0.017 * SR as f64).round() as usize; // 816 @48k
        assert!((buf[tap] - 0.175).abs() < 1e-3, "tap1 = {}", buf[tap]);
        assert!(
            buf[1..tap].iter().all(|s| *s == 0.0),
            "silent before first reflection"
        );
    }
}
