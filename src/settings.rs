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
pub const SIMPLE_BANDS: usize = 5;
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

/// Which FxSound-style slider effect to control.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FxKind {
    Clarity,
    Surround,
    Ambience,
    DynamicBoost,
    BassBoost,
}

pub const AMOUNT_MIN: f32 = 0.0;
pub const AMOUNT_MAX: f32 = 100.0;

/// One FxSound-style effect: on/off toggle plus a continuous 0..100 slider. The five
/// of these stack alongside (not instead of) the bass/vocal/treble boost sections.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fx {
    pub enabled: bool,
    pub amount: f32,
}

impl Fx {
    pub fn new(enabled: bool, amount: f32) -> Self {
        Fx { enabled, amount }
    }

    fn sanitized(&self) -> Self {
        Fx {
            enabled: self.enabled,
            amount: self.amount.clamp(AMOUNT_MIN, AMOUNT_MAX),
        }
    }
}

impl Default for Fx {
    fn default() -> Self {
        Fx {
            enabled: false,
            amount: 0.0,
        }
    }
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
        Band::new(400.0, 0.0, 1.0),  // peaking
        Band::new(1000.0, 0.0, 1.0), // peaking (mid)
        Band::new(2500.0, 0.0, 1.0), // peaking
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
    /// Fidelity: presence/detail lift (peaking).
    pub clarity: Fx,
    /// Stereo width enhancement.
    pub surround: Fx,
    /// Early-reflection ambience.
    pub ambience: Fx,
    /// Upward-compressor lift for quiet material.
    pub dynamic_boost: Fx,
    /// Low-shelf bass boost slider; stacks with the `bass` boost section.
    pub bass_boost: Fx,
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
            clarity: Fx::default(),
            surround: Fx::default(),
            ambience: Fx::default(),
            dynamic_boost: Fx::default(),
            bass_boost: Fx::default(),
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
        self.clarity = self.clarity.sanitized();
        self.surround = self.surround.sanitized();
        self.ambience = self.ambience.sanitized();
        self.dynamic_boost = self.dynamic_boost.sanitized();
        self.bass_boost = self.bass_boost.sanitized();
        // old 3-band configs don't map onto the 5-band layout: rebuild flat
        if self.simple_bands.len() != SIMPLE_BANDS {
            self.simple_bands = default_simple_bands();
        }
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

    pub fn fx(&self, kind: FxKind) -> &Fx {
        match kind {
            FxKind::Clarity => &self.clarity,
            FxKind::Surround => &self.surround,
            FxKind::Ambience => &self.ambience,
            FxKind::DynamicBoost => &self.dynamic_boost,
            FxKind::BassBoost => &self.bass_boost,
        }
    }

    pub fn fx_mut(&mut self, kind: FxKind) -> &mut Fx {
        match kind {
            FxKind::Clarity => &mut self.clarity,
            FxKind::Surround => &mut self.surround,
            FxKind::Ambience => &mut self.ambience,
            FxKind::DynamicBoost => &mut self.dynamic_boost,
            FxKind::BassBoost => &mut self.bass_boost,
        }
    }

    /// Static control schema for a future GUI: every widget it should build, with ranges.
    /// Values themselves come from `status`/`--json settings`.
    pub fn describe() -> serde_json::Value {
        fn tog(key: &str) -> serde_json::Value {
            serde_json::json!({"key": key, "type": "toggle"})
        }
        fn slider(key: &str, min: f32, max: f32, step: f32, unit: &str) -> serde_json::Value {
            serde_json::json!({"key": key, "type": "slider", "min": min, "max": max, "step": step, "unit": unit})
        }
        let mut boost_controls = Vec::new();
        for sec in ["bass", "vocal", "treble"] {
            boost_controls.push(tog(&format!("{sec}.enabled")));
            boost_controls.push(slider(
                &format!("{sec}.gain_db"),
                BOOST_GAIN_MIN,
                BOOST_GAIN_MAX,
                0.5,
                "dB",
            ));
            boost_controls.push(slider(
                &format!("{sec}.freq_hz"),
                FREQ_MIN,
                FREQ_MAX,
                1.0,
                "Hz",
            ));
            boost_controls.push(slider(&format!("{sec}.q"), Q_MIN, Q_MAX, 0.1, ""));
        }
        let mut fx_controls = Vec::new();
        for key in [
            "clarity",
            "surround",
            "ambience",
            "dynamic_boost",
            "bass_boost",
        ] {
            fx_controls.push(tog(&format!("{key}.enabled")));
            fx_controls.push(slider(
                &format!("{key}.amount"),
                AMOUNT_MIN,
                AMOUNT_MAX,
                1.0,
                "%",
            ));
        }
        let mut simple_controls = vec![
            serde_json::json!({"key": "eq_mode", "type": "enum", "values": ["simple", "multi"]}),
        ];
        for (i, name) in ["low", "low-mid", "mid", "high-mid", "high"]
            .iter()
            .enumerate()
        {
            simple_controls.push(slider(
                &format!("simple_bands.{i}.freq_hz"),
                FREQ_MIN,
                FREQ_MAX,
                1.0,
                "Hz",
            ));
            simple_controls.push(slider(
                &format!("simple_bands.{i}.gain_db"),
                GAIN_MIN,
                GAIN_MAX,
                0.5,
                "dB",
            ));
            simple_controls.push(slider(
                &format!("simple_bands.{i}.q"),
                Q_MIN,
                Q_MAX,
                0.1,
                "",
            ));
            simple_controls.last_mut().unwrap()["label"] = serde_json::json!(name);
        }
        serde_json::json!({
            "version": 1,
            "groups": [
                {"name": "master", "controls": [
                    tog("bypass"),
                    slider("preamp_db", PREAMP_MIN, PREAMP_MAX, 0.5, "dB"),
                    tog("limiter"),
                ]},
                {"name": "boosts", "controls": boost_controls},
                {"name": "fx", "controls": fx_controls},
                {"name": "eq_simple", "controls": simple_controls},
                {"name": "eq_multi", "controls": [
                    {"key": "multi_bands", "type": "band_count", "min": MULTI_BANDS_MIN, "max": MULTI_BANDS_MAX},
                    slider("multi_bands.[].freq_hz", FREQ_MIN, FREQ_MAX, 1.0, "Hz"),
                    slider("multi_bands.[].gain_db", GAIN_MIN, GAIN_MAX, 0.5, "dB"),
                    slider("multi_bands.[].q", Q_MIN, Q_MAX, 0.1, ""),
                ]},
            ]
        })
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
    if name.starts_with('.') || name.contains(['/', '\\']) || name.chars().any(|c| c.is_control()) {
        return Err(
            "preset name may not start with '.' or contain path separators/control chars".into(),
        );
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
    if path.exists() {
        let mut s = load_settings(&path);
        s.sanitize();
        return Ok(s);
    }
    // user preset missing: fall back to the shipped FxSound factory presets
    crate::factory::load(name).ok_or_else(|| format!("preset '{name}' not found"))
}

/// Delete a user preset; built-in factory presets stay protected
/// (a user file shadowing a factory name is removed normally).
pub fn delete_preset(name: &str) -> Result<(), String> {
    validate_preset_name(name)?;
    let path = presets_dir().join(format!("{name}.toml"));
    if path.exists() {
        return std::fs::remove_file(&path).map_err(|e| e.to_string());
    }
    if crate::factory::load(name).is_some() {
        return Err(format!("'{name}' is a built-in preset"));
    }
    Err(format!("preset '{name}' not found"))
}

/// Rename a user preset; built-in factory presets stay protected.
pub fn rename_preset(from: &str, to: &str) -> Result<(), String> {
    validate_preset_name(from)?;
    validate_preset_name(to)?;
    let src = presets_dir().join(format!("{from}.toml"));
    if !src.exists() {
        if crate::factory::load(from).is_some() {
            return Err(format!(
                "'{from}' is a built-in preset; save it under a new name first"
            ));
        }
        return Err(format!("preset '{from}' not found"));
    }
    let dst = presets_dir().join(format!("{to}.toml"));
    if dst.exists() {
        return Err(format!("preset '{to}' already exists"));
    }
    std::fs::rename(&src, &dst).map_err(|e| e.to_string())
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
    for name in crate::factory::names() {
        if !out.contains(&name) {
            out.push(name);
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
    fn preset_names_allow_fxspell_names() {
        assert!(validate_preset_name("Volume Boost").is_ok());
        assert!(validate_preset_name("70's").is_ok());
        assert!(validate_preset_name("R&B").is_ok());
        assert!(validate_preset_name(".hidden").is_err());
        assert!(validate_preset_name("a/b").is_err());
        assert!(validate_preset_name("bad\\name").is_err());
    }

    #[test]
    fn old_config_without_fx_fields_still_loads() {
        let old = "bypass = false\npreamp_db = -3.0\nlimiter = true\neq_mode = \"simple\"\n\n[bass]\nenabled = true\ngain_db = 6.0\nfreq_hz = 150.0\n";
        let s: Settings = toml::from_str(old).unwrap();
        assert_eq!(s.preamp_db, -3.0);
        assert!(s.bass.enabled);
        assert_eq!(s.clarity, Fx::default());
        assert_eq!(s.bass_boost, Fx::default());
        assert_eq!(s.dynamic_boost, Fx::default());
    }

    #[test]
    fn factory_presets_list_and_load() {
        let list = list_presets().unwrap();
        assert!(list.contains(&"General".to_string()));
        assert!(list.contains(&"Music".to_string()));
        let s = load_preset("General").unwrap();
        assert!(s.clarity.enabled);
        assert_eq!(s.clarity.amount, 39.0);
    }

    #[test]
    fn describe_exposes_fx_schema() {
        let d = Settings::describe();
        let groups = d["groups"].as_array().unwrap();
        let names: Vec<&str> = groups.iter().filter_map(|g| g["name"].as_str()).collect();
        assert_eq!(
            names,
            vec!["master", "boosts", "fx", "eq_simple", "eq_multi"]
        );
        let fx = &groups[2];
        assert_eq!(fx["controls"].as_array().unwrap().len(), 10); // 5 effects x (toggle+slider)
        let amount = &fx["controls"][1];
        assert_eq!(amount["type"], "slider");
        assert_eq!(amount["max"], 100.0);
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
