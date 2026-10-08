//! FxSound `.fac` preset import: text format of Main values, app integers and the 10-band EQ.
//! FxSound stores effect levels as MIDI ints (0..127); their quantizer maps that linearly to a
//! 0..10 UI range, which we express as our 0..100 slider.

use crate::settings::{Band, Fx, MULTI_BANDS_MAX, Settings};

/// One parsed FxSound preset. Main order per `u_DfxDsp.h` param maps:
/// [0] Fidelity, [1] Surround, [2] unused, [3] Ambience, [4] DynamicBoost, [5] Bass.
/// App integers: [0] Fidelity, [1] Surround, [2] Ambience, [3] Dynamic, [4] Bass, [5] Headphone, [6] MusicMode.
#[derive(Debug, Clone, PartialEq)]
pub struct FacPreset {
    pub name: String,
    pub mains: [i64; 6],
    pub ints: Vec<i64>,
    pub eq_on: bool,
    pub eq: Vec<Band>,
}

impl FacPreset {
    /// Map a FxSound preset onto our settings: five slider effects + their 10-band EQ.
    /// Headphone/MusicMode flags and the unused Main are intentionally dropped.
    pub fn to_settings(&self) -> Settings {
        let int = |i: usize| self.ints.get(i).copied().unwrap_or(0) == 1;
        let mut s = Settings::default();
        s.clarity = Fx::new(int(0), midi_to_amount(self.mains[0]));
        s.surround = Fx::new(int(1), midi_to_amount(self.mains[1]));
        s.ambience = Fx::new(int(2), midi_to_amount(self.mains[3]));
        s.dynamic_boost = Fx::new(int(3), midi_to_amount(self.mains[4]));
        s.bass_boost = Fx::new(int(4), midi_to_amount(self.mains[5]));
        s.eq_mode = crate::settings::EqMode::Multi;
        s.multi_bands = self.eq.clone();
        s.multi_bands.truncate(MULTI_BANDS_MAX);
        if !self.eq_on {
            for b in &mut s.multi_bands {
                b.gain_db = 0.0;
            }
        }
        s.sanitize();
        s
    }
}

/// Linear MIDI 0..127 -> slider 0..100 (integer, FxSound sliders are 0..100).
pub fn midi_to_amount(midi: i64) -> f32 {
    (midi.clamp(0, 127) as f32 * 100.0 / 127.0).round()
}

/// Parse a `.fac` text preset. Unknown lines are ignored, so older/newer file versions load.
pub fn parse_fac(text: &str) -> Result<FacPreset, String> {
    let mut name = String::new();
    let mut mains = [0i64; 6];
    let mut ints: Vec<i64> = Vec::new();
    let mut eq_on = false;
    let mut bands: Vec<Band> = Vec::new();
    let mut pending_cf: Option<f64> = None;

    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if i == 2 && !line.is_empty() && !line.contains(": ") {
            name = line.to_string();
            continue;
        }
        let Some((num, label)) = line.split_once(": ") else {
            continue;
        };
        let Ok(num) = num.trim().parse::<f64>() else {
            continue; // "CLASS1" header and other non-numeric lines
        };
        if let Some(rest) = label.strip_prefix("Main ") {
            if let Ok(idx) = rest.parse::<usize>() {
                if idx < mains.len() {
                    mains[idx] = num as i64;
                }
            }
        } else if let Some(rest) = label.strip_prefix("Integer[") {
            let idx: usize = rest.trim_end_matches(']').parse().unwrap_or(0);
            if idx < 64 {
                if ints.len() <= idx {
                    ints.resize(idx + 1, 0);
                }
                ints[idx] = num as i64;
            }
        } else if label == "On/Off Flag" {
            eq_on = num != 0.0;
        } else if label == "CF" {
            pending_cf = Some(num);
        } else if label == "Boost/Cut" {
            if let Some(cf) = pending_cf.take() {
                bands.push(Band::new(cf.clamp(10.0, 24_000.0) as f32, num as f32, 1.4));
            }
        }
    }

    if name.is_empty() {
        return Err("preset has no name (line 3)".into());
    }
    Ok(FacPreset {
        name,
        mains,
        ints,
        eq_on,
        eq: bands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAC1: &str = include_str!("../presets-fx/1.fac");

    #[test]
    fn parses_factory_general() {
        let p = parse_fac(FAC1).unwrap();
        assert_eq!(p.name, "General");
        assert_eq!(p.mains, [50, 20, 0, 0, 60, 60]);
        assert_eq!(p.ints, vec![1, 1, 0, 1, 1, 0, 2]); // [6] = music mode
        assert!(p.eq_on);
        assert_eq!(p.eq.len(), 10);
        assert!((p.eq[0].freq_hz - 62.5).abs() < 0.01);
        assert_eq!(p.eq[0].gain_db, 0.0);
        assert_eq!(p.eq[3].gain_db, 2.0);
        assert_eq!(p.eq[8].gain_db, -2.0);
    }

    #[test]
    fn maps_effects_to_settings() {
        let s = parse_fac(FAC1).unwrap().to_settings();
        assert!(s.clarity.enabled);
        assert_eq!(s.clarity.amount, 39.0); // 50/127*100 rounded
        assert!(s.surround.enabled);
        assert_eq!(s.surround.amount, 16.0);
        assert!(!s.ambience.enabled);
        assert!(s.dynamic_boost.enabled);
        assert_eq!(s.dynamic_boost.amount, 47.0);
        assert!(s.bass_boost.enabled);
        assert_eq!(s.bass_boost.amount, 47.0);
        // FxSound presets carry no 3-band boosts; ours stay off.
        assert!(!s.bass.enabled);
        assert!(!s.vocal.enabled);
        assert!(!s.treble.enabled);
        assert!(s.limiter);
        assert!(!s.bypass);
        assert_eq!(s.multi_bands.len(), 10);
    }

    #[test]
    fn eq_off_zeroes_gains() {
        let mut p = parse_fac(FAC1).unwrap();
        p.eq_on = false;
        let s = p.to_settings();
        assert!(s.multi_bands.iter().all(|b| b.gain_db == 0.0));
        assert!((s.multi_bands[3].freq_hz - 450.0).abs() < 0.01); // freqs kept
    }

    #[test]
    fn rejects_nameless_file() {
        let text = "CLASS1 : Effect Type\n9: Version\n\n0: Double Params Flag\n";
        assert!(parse_fac(text).is_err());
    }

    #[test]
    fn midi_roundtrip_endpoints() {
        assert_eq!(midi_to_amount(0), 0.0);
        assert_eq!(midi_to_amount(127), 100.0);
        assert_eq!(midi_to_amount(-5), 0.0);
        assert_eq!(midi_to_amount(200), 100.0);
    }
}
