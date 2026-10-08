//! User-facing settings: boosts, two EQ modes, presets, config persistence.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FREQ_MIN: f32 = 20.0;
pub const FREQ_MAX: f32 = 20_000.0;
pub const GAIN_MIN: f32 = -20.0;
pub const GAIN_MAX: f32 = 20.0;
pub const BOOST_GAIN_MIN: f32 = -12.0;
pub const BOOST_GAIN_MAX: f32 = 12.0;
pub const PREAMP_MIN: f32 = -24.0;
pub const PREAMP_MAX: f32 = 12.0;
pub const Q_MIN: f32 = 0.1;
pub const Q_MAX: f32 = 30.0;
pub const SIMPLE_BANDS: usize = 3;
pub const MULTI_BANDS_MIN: usize = 1;
pub const MULTI_BANDS_MAX: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EqMode {
    #[default]
    Simple,
    Multi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoostKind {
    Bass,
    Vocal,
    Treble,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub freq_hz: f32,
    pub gain_db: f32,
    #[serde(default = "default_q")]
    pub q: f32,
}

fn default_q() -> f32 {
    1.0
}

impl Band {
    pub fn new(freq_hz: f32, gain_db: f32, q: f32) -> Self {
        Self {
            freq_hz,
            gain_db,
            q,
        }
    }

    fn sanitized(&self) -> Self {
        Self {
            freq_hz: self.freq_hz.clamp(FREQ_MIN, FREQ_MAX),
            gain_db: self.gain_db.clamp(GAIN_MIN, GAIN_MAX),
            q: self.q.clamp(Q_MIN, Q_MAX),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Boost {
    pub enabled: bool,
    pub gain_db: f32,
    pub freq_hz: f32,
    #[serde(default = "default_boost_q")]
    pub q: f32,
}

fn default_boost_q() -> f32 {
    1.0
}

impl Boost {
    fn sanitized(&self) -> Self {
        Self {
            enabled: self.enabled,
            gain_db: self.gain_db.clamp(BOOST_GAIN_MIN, BOOST_GAIN_MAX),
            freq_hz: self.freq_hz.clamp(FREQ_MIN, FREQ_MAX),
            q: self.q.clamp(Q_MIN, Q_MAX),
        }
    }
}

fn default_bass() -> Boost {
    Boost {
        enabled: false,
        gain_db: 6.0,
        freq_hz: 150.0,
        q: 0.7,
    }
}

fn default_vocal() -> Boost {
    Boost {
        enabled: false,
        gain_db: 4.0,
        freq_hz: 3000.0,
        q: 1.0,
    }
}

fn default_treble() -> Boost {
    Boost {
        enabled: false,
        gain_db: 4.0,
        freq_hz: 9000.0,
        q: 0.7,
    }
}

fn default_simple_bands() -> Vec<Band> {
    vec![
        Band::new(120.0, 0.0, 0.7),  // low shelf
        Band::new(1000.0, 0.0, 1.0), // peaking (mid)
        Band::new(8000.0, 0.0, 0.7), // high shelf
    ]
}

fn default_multi_bands() -> Vec<Band> {
    [
        31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
    ]
    .iter()
    .map(|&f| Band::new(f, 0.0, 1.4))
    .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub bypass: bool,
    pub preamp_db: f32,
    pub limiter: bool,
    pub bass: Boost,
    pub vocal: Boost,
    pub treble: Boost,
    pub eq_mode: EqMode,
    /// 3 bands: low shelf, peaking mid, high shelf (order fixed).
    pub simple_bands: Vec<Band>,
    /// N peaking bands, count adjustable at runtime.
    pub multi_bands: Vec<Band>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bypass: false,
            preamp_db: 0.0,
            limiter: true,
            bass: default_bass(),
            vocal: default_vocal(),
            treble: default_treble(),
            eq_mode: EqMode::Simple,
            simple_bands: default_simple_bands(),
            multi_bands: default_multi_bands(),
        }
    }
}

impl Settings {
    /// Clamp every field into its valid range and fix list lengths.
    pub fn sanitize(&mut self) {
        self.preamp_db = self.preamp_db.clamp(PREAMP_MIN, PREAMP_MAX);
        self.bass = self.bass.sanitized();
        self.vocal = self.vocal.sanitized();
        self.treble = self.treble.sanitized();
        while self.simple_bands.len() < SIMPLE_BANDS {
            let defaults = default_simple_bands();
            self.simple_bands.push(defaults[self.simple_bands.len()]);
        }
        self.simple_bands.truncate(SIMPLE_BANDS);
        for b in &mut self.simple_bands {
            *b = b.sanitized();
        }
        if self.multi_bands.is_empty() {
            self.multi_bands = default_multi_bands();
        }
        self.multi_bands.truncate(MULTI_BANDS_MAX);
        for b in &mut self.multi_bands {
            *b = b.sanitized();
        }
    }

    pub fn band(&self, mode: EqMode, index: usize) -> Option<&Band> {
        match mode {
            EqMode::Simple => self.simple_bands.get(index),
            EqMode::Multi => self.multi_bands.get(index),
        }
    }

    pub fn set_band(&mut self, mode: EqMode, index: usize, band: Band) -> bool {
        match mode {
            EqMode::Simple => {
                if let Some(slot) = self.simple_bands.get_mut(index) {
                    *slot = band;
                    true
                } else {
                    false
                }
            }
            EqMode::Multi => {
                if let Some(slot) = self.multi_bands.get_mut(index) {
                    *slot = band;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Resize multi-band EQ, preserving existing band values where possible.
    pub fn set_band_count(&mut self, count: usize) {
        let count = count.clamp(MULTI_BANDS_MIN, MULTI_BANDS_MAX);
        let defaults = default_multi_bands();
        while self.multi_bands.len() < count {
            let i = self.multi_bands.len();
            // spread defaults across whatever count was asked for
            let src = if defaults.len() >= count {
                i * defaults.len() / count
            } else {
                i * (defaults.len() - 1) / count.max(1)
            };
            self.multi_bands.push(defaults[src.min(defaults.len() - 1)]);
        }
        self.multi_bands.truncate(count);
    }

    pub fn boost(&self, kind: BoostKind) -> &Boost {
        match kind {
            BoostKind::Bass => &self.bass,
            BoostKind::Vocal => &self.vocal,
            BoostKind::Treble => &self.treble,
        }
    }

    pub fn boost_mut(&mut self, kind: BoostKind) -> &mut Boost {
        match kind {
            BoostKind::Bass => &mut self.bass,
            BoostKind::Vocal => &mut self.vocal,
            BoostKind::Treble => &mut self.treble,
        }
    }
}

// ---------- config / preset paths ----------

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        });
    base.join("anontokyo")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn presets_dir() -> PathBuf {
    config_dir().join("presets")
}

fn validate_preset_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("preset name must be 1-64 chars".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("preset name may only contain [A-Za-z0-9_-]".into());
    }
    Ok(())
}

pub fn load_settings(path: &Path) -> Settings {
    let mut settings = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str::<Settings>(&text).unwrap_or_else(|e| {
            log::warn!("config {path:?} unreadable ({e}); using defaults");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    };
    settings.sanitize();
    settings
}

pub fn save_settings(path: &Path, settings: &Settings) -> Result<(), String> {
    let dir = path.parent().ok_or("config path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = toml::to_string_pretty(settings).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn save_preset(name: &str, settings: &Settings) -> Result<(), String> {
    validate_preset_name(name)?;
    let dir = presets_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    save_settings(&dir.join(format!("{name}.toml")), settings)
}

pub fn load_preset(name: &str) -> Result<Settings, String> {
    validate_preset_name(name)?;
    let path = presets_dir().join(format!("{name}.toml"));
    if !path.exists() {
        return Err(format!("preset '{name}' not found"));
    }
    let mut s = load_settings(&path);
    s.sanitize();
    Ok(s)
}

pub fn list_presets() -> Result<Vec<String>, String> {
    let dir = presets_dir();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "toml") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    out.push(stem.to_string());
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_everything() {
        let mut s = Settings {
            preamp_db: 999.0,
            bass: Boost {
                enabled: true,
                gain_db: 500.0,
                freq_hz: 1.0,
                q: 0.0,
            },
            simple_bands: vec![Band::new(99_999.0, -999.0, 0.0)],
            multi_bands: vec![],
            ..Settings::default()
        };
        s.sanitize();
        assert_eq!(s.preamp_db, PREAMP_MAX);
        assert_eq!(s.bass.gain_db, BOOST_GAIN_MAX);
        assert_eq!(s.bass.freq_hz, FREQ_MIN);
        assert_eq!(s.bass.q, Q_MIN);
        assert_eq!(s.simple_bands.len(), SIMPLE_BANDS);
        assert_eq!(s.multi_bands.len(), 10);
        assert!(
            s.multi_bands
                .iter()
                .all(|b| (FREQ_MIN..=FREQ_MAX).contains(&b.freq_hz))
        );
    }

    #[test]
    fn band_count_resize_preserves_values() {
        let mut s = Settings::default();
        s.multi_bands[0].gain_db = 5.0;
        s.set_band_count(16);
        assert_eq!(s.multi_bands.len(), 16);
        assert_eq!(s.multi_bands[0].gain_db, 5.0);
        s.set_band_count(4);
        assert_eq!(s.multi_bands.len(), 4);
        s.set_band_count(999);
        assert_eq!(s.multi_bands.len(), MULTI_BANDS_MAX);
        s.set_band_count(0);
        assert_eq!(s.multi_bands.len(), MULTI_BANDS_MIN);
    }

    fn test_dir(name: &str) -> PathBuf {
        // never use the system temp dir on this machine (RAM tmpfs, shared)
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!("{name}-{}", std::process::id()))
    }

    #[test]
    fn config_roundtrip() {
        let dir = test_dir("roundtrip");
        let path = dir.join("config.toml");
        let mut s = Settings::default();
        s.bass.enabled = true;
        s.bass.gain_db = 7.5;
        s.eq_mode = EqMode::Multi;
        save_settings(&path, &s).unwrap();
        let back = load_settings(&path);
        assert_eq!(s, back);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preset_names_are_path_traversal_safe() {
        assert!(save_preset("../evil", &Settings::default()).is_err());
        assert!(load_preset("../../etc/passwd").is_err());
        assert!(save_preset("", &Settings::default()).is_err());
    }

    #[test]
    fn corrupt_config_falls_back_to_defaults() {
        let dir = test_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "this is not toml [[[").unwrap();
        let s = load_settings(&path);
        assert_eq!(s, Settings::default());
        std::fs::remove_dir_all(&dir).ok();
    }
}
