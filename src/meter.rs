//! Live spectrum meter for the UI visualizer.
//!
//! The audio thread runs a Goertzel filter bank over a Hann-windowed
//! snapshot and stores 0..1 band levels as atomics; the control
//! thread reads them when the UI polls `Meter`.

use std::sync::atomic::{AtomicU32, Ordering};

pub const METER_BANDS: usize = 24;
const WINDOW: usize = 1024;
const LOWEST_HZ: f32 = 30.0;
const HIGHEST_HZ: f32 = 16_000.0;
// capture taps the sink monitor post-volume; quiet listening lands near -70 dB
const FLOOR_DB: f32 = -96.0;

/// Log-spaced band centre frequencies, shared by analyzer and labels.
pub fn band_freqs() -> [f32; METER_BANDS] {
    std::array::from_fn(|i| {
        let t = i as f32 / (METER_BANDS - 1) as f32;
        LOWEST_HZ * (HIGHEST_HZ / LOWEST_HZ).powf(t)
    })
}

/// Lock-free band levels: audio thread writes, control thread reads.
pub struct Meter {
    levels: [AtomicU32; METER_BANDS],
}

impl Meter {
    pub fn new() -> Self {
        Self {
            levels: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
        }
    }

    pub fn store(&self, levels: &[f32; METER_BANDS]) {
        for (slot, v) in self.levels.iter().zip(levels) {
            slot.store(v.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn levels(&self) -> [f32; METER_BANDS] {
        std::array::from_fn(|i| f32::from_bits(self.levels[i].load(Ordering::Relaxed)))
    }
}

/// Real-time-side analyzer; lives in the capture stream's user data.
pub struct MeterAnalyzer {
    hann: Vec<f32>,
    freqs: [f32; METER_BANDS],
    mono: Vec<f32>,
    out: [f32; METER_BANDS],
    rate: f32,
}

impl MeterAnalyzer {
    pub fn new(rate: u32) -> Self {
        let hann: Vec<f32> = (0..WINDOW)
            .map(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (WINDOW - 1) as f32).cos()
            })
            .collect();
        Self {
            hann,
            freqs: band_freqs(),
            mono: vec![0.0; WINDOW],
            out: [0.0; METER_BANDS],
            rate: rate as f32,
        }
    }

    /// Downmix one interleaved chunk to mono and return band levels.
    pub fn analyze(&mut self, buf: &[f32], channels: usize) -> &[f32; METER_BANDS] {
        let ch = channels.max(1);
        let frames = (buf.len() / ch).min(WINDOW);
        if frames == 0 {
            return &self.out;
        }
        for f in 0..frames {
            let base = f * ch;
            let mut acc = 0.0;
            for c in 0..ch {
                acc += buf[base + c];
            }
            self.mono[f] = acc / ch as f32;
        }
        self.mono[frames..].fill(0.0);

        let norm: f32 = self.hann.iter().sum::<f32>() * 0.5;
        for (b, &fc) in self.freqs.iter().enumerate() {
            let w = 2.0 * std::f32::consts::PI * fc / self.rate;
            let coeff = 2.0 * w.cos() as f64;
            let (mut s1, mut s2) = (0.0f64, 0.0f64);
            for i in 0..frames {
                let x = (self.mono[i] * self.hann[i]) as f64;
                let s0 = x + coeff * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            let re = s1 - s2 * w.cos() as f64;
            let im = s2 * w.sin() as f64;
            let mag = ((re * re + im * im).sqrt() as f32) / norm;
            let db = 20.0 * (mag + 1e-6).log10();
            self.out[b] = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
        }
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn tone_lights_its_band_only() {
        let mut a = MeterAnalyzer::new(48_000);
        let buf = sine(440.0, 0.5, 48_000, WINDOW);
        let levels = a.analyze(&buf, 1);
        let (band, level) = levels
            .iter()
            .copied()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .unwrap();
        // 440 Hz sits between the 348 Hz and 458 Hz band centres
        assert!((9..=10).contains(&band), "peak band {band}");
        assert!(level > 0.7, "peak level {level}");
        assert!(levels[METER_BANDS - 1] < 0.05);
    }

    #[test]
    fn silence_reads_zero() {
        let mut a = MeterAnalyzer::new(48_000);
        let levels = a.analyze(&vec![0.0; WINDOW], 1);
        assert!(levels.iter().all(|&l| l < 0.01));
    }

    #[test]
    fn stereo_downmix() {
        let mut a = MeterAnalyzer::new(48_000);
        let mono = sine(1000.0, 0.5, 48_000, WINDOW);
        let mut interleaved = Vec::with_capacity(WINDOW * 2);
        for &s in &mono {
            interleaved.push(s);
            interleaved.push(s);
        }
        let l = a.analyze(&interleaved, 2);
        let (band, level) = l
            .iter()
            .copied()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .unwrap();
        // 1 kHz nearest centre is 1044 Hz at band 13
        assert_eq!(band, 13, "1 kHz should peak at band 13, got {band}");
        assert!(level > 0.7);
    }

    #[test]
    fn meter_store_roundtrip() {
        let m = Meter::new();
        let mut want = [0.0f32; METER_BANDS];
        want[3] = 0.5;
        want[20] = 1.0;
        m.store(&want);
        assert_eq!(m.levels(), want);
    }

    #[test]
    fn band_freqs_are_log_spaced() {
        let f = band_freqs();
        assert!((f[0] - LOWEST_HZ).abs() < 0.01);
        assert!(f[METER_BANDS - 1] > HIGHEST_HZ * 0.99);
        assert!(f.windows(2).all(|w| w[1] > w[0]));
    }
}
