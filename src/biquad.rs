//! RBJ audio-EQ-cookbook biquad filters.
//!
//! All coefficient math here is f64; audio samples go through as f32 and are
//! converted at the boundary. Coefficients are sanitized against the sample
//! rate at construction so a bad config value can never make an unstable filter.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BiquadState {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    fn from_raw(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    fn sanitize(sample_rate: f64, freq: f64, q: f64) -> (f64, f64, f64) {
        let nyquist = sample_rate * 0.5;
        let freq = freq.clamp(10.0, nyquist * 0.99);
        let q = q.clamp(0.05, 50.0);
        (freq, q, 2.0 * std::f64::consts::PI * freq / sample_rate)
    }

    pub fn peaking(sample_rate: f64, freq: f64, gain_db: f64, q: f64) -> Self {
        let (freq, q, w0) = Self::sanitize(sample_rate, freq, q);
        let _ = freq;
        let a = 10.0_f64.powf(gain_db / 40.0);
        let alpha = w0.sin() / (2.0 * q);
        let cos = w0.cos();
        Self::from_raw(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        )
    }

    pub fn low_shelf(sample_rate: f64, freq: f64, gain_db: f64) -> Self {
        let (freq, _, w0) = Self::sanitize(sample_rate, freq, 0.707);
        let _ = freq;
        let a = 10.0_f64.powf(gain_db / 40.0);
        // S = 1: alpha = sin(w0)/2 * sqrt((A + 1/A)*(1/S - 1) + 2)
        let alpha = w0.sin() / 2.0 * 2.0_f64.sqrt();
        let cos = w0.cos();
        let sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        Self::from_raw(
            a * ((a + 1.0) - (a - 1.0) * cos + sqrt_a_alpha),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - sqrt_a_alpha),
            (a + 1.0) + (a - 1.0) * cos + sqrt_a_alpha,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - sqrt_a_alpha,
        )
    }

    pub fn high_shelf(sample_rate: f64, freq: f64, gain_db: f64) -> Self {
        let (freq, _, w0) = Self::sanitize(sample_rate, freq, 0.707);
        let _ = freq;
        let a = 10.0_f64.powf(gain_db / 40.0);
        let alpha = w0.sin() / 2.0 * 2.0_f64.sqrt();
        let cos = w0.cos();
        let sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        Self::from_raw(
            a * ((a + 1.0) + (a - 1.0) * cos + sqrt_a_alpha),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - sqrt_a_alpha),
            (a + 1.0) - (a - 1.0) * cos + sqrt_a_alpha,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - sqrt_a_alpha,
        )
    }

    /// Direct form I, one sample.
    #[inline]
    pub fn process_sample(&self, state: &mut BiquadState, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * state.x1 + self.b2 * state.x2
            - self.a1 * state.y1
            - self.a2 * state.y2;
        state.x2 = state.x1;
        state.x1 = x;
        state.y2 = state.y1;
        state.y1 = y;
        y
    }

    /// Magnitude response in dB at `freq`.
    pub fn magnitude_db(&self, freq: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / sample_rate;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let (num_re, num_im) = (
            self.b0 + self.b1 * c1 + self.b2 * c2,
            -(self.b1 * s1 + self.b2 * s2),
        );
        let (den_re, den_im) = (
            1.0 + self.a1 * c1 + self.a2 * c2,
            -(self.a1 * s1 + self.a2 * s2),
        );
        let mag2 = (num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im);
        10.0 * mag2.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    #[test]
    fn peaking_hits_configured_gain_at_center() {
        for gain in [-12.0, -6.0, 0.0, 3.0, 9.0] {
            let f = Biquad::peaking(SR, 1000.0, gain, 1.0);
            let got = f.magnitude_db(1000.0, SR);
            assert!((got - gain).abs() < 0.01, "gain {gain}: got {got}");
        }
    }

    #[test]
    fn peaking_flat_away_from_center() {
        let f = Biquad::peaking(SR, 1000.0, 12.0, 2.0);
        let low = f.magnitude_db(50.0, SR);
        let high = f.magnitude_db(18_000.0, SR);
        assert!(low.abs() < 0.5, "low {low}");
        assert!(high.abs() < 0.5, "high {high}");
    }

    #[test]
    fn low_shelf_boosts_lows_and_passes_highs() {
        let f = Biquad::low_shelf(SR, 200.0, 10.0);
        let bass = f.magnitude_db(30.0, SR);
        let mid = f.magnitude_db(200.0, SR);
        let treble = f.magnitude_db(10_000.0, SR);
        assert!((bass - 10.0).abs() < 0.5, "bass {bass}");
        assert!((mid - 5.0).abs() < 1.0, "mid {mid}");
        assert!(treble.abs() < 0.5, "treble {treble}");
    }

    #[test]
    fn high_shelf_boosts_treble_and_passes_lows() {
        let f = Biquad::high_shelf(SR, 8000.0, 8.0);
        let low = f.magnitude_db(100.0, SR);
        let at = f.magnitude_db(8000.0, SR);
        let high = f.magnitude_db(18_000.0, SR);
        assert!(low.abs() < 0.5, "low {low}");
        assert!((at - 4.0).abs() < 1.0, "at {at}");
        assert!((high - 8.0).abs() < 0.5, "high {high}");
    }

    #[test]
    fn zero_gain_filters_are_identity_in_steady_state() {
        let f = Biquad::peaking(SR, 1000.0, 0.0, 1.0);
        // unity numerator == denominator, response must be exactly flat
        for hz in [60.0, 1000.0, 12_000.0] {
            assert!(f.magnitude_db(hz, SR).abs() < 1e-9);
        }
    }

    #[test]
    fn out_of_range_values_do_not_explode() {
        // deliberately absurd config values must still produce sane coefficients
        let f = Biquad::peaking(SR, 999_999.0, 40.0, 0.0);
        assert!(f.b0.is_finite() && f.a1.is_finite() && f.a2.is_finite());
        let g = Biquad::low_shelf(SR, -50.0, -60.0);
        assert!(g.b0.is_finite() && g.a2.is_finite());
    }

    #[test]
    fn sine_through_shelf_settles_to_predicted_gain() {
        // time-domain sanity: measure steady-state RMS against magnitude_db
        let target_db = 6.0;
        let f = Biquad::low_shelf(SR, 150.0, target_db);
        let hz = 60.0;
        let mut state = BiquadState::default();
        let mut peak_in = 0.0f64;
        let mut peak_out = 0.0f64;
        for n in 0..(SR as usize * 2) {
            let x = (2.0 * std::f64::consts::PI * hz * n as f64 / SR).sin();
            let y = f.process_sample(&mut state, x);
            if n > SR as usize {
                peak_in = peak_in.max(x.abs());
                peak_out = peak_out.max(y.abs());
            }
        }
        let measured_db = 20.0 * (peak_out / peak_in).log10();
        let predicted = f.magnitude_db(hz, SR);
        assert!(
            (measured_db - predicted).abs() < 0.1,
            "measured {measured_db}, predicted {predicted}"
        );
    }
}
