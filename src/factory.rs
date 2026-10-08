//! The twelve FxSound factory presets, shipped as their original `.fac` files and parsed
//! on demand (data derived from the FxSound project, https://github.com/fxsoundapp/fxsound).

use crate::fac::parse_fac;
use crate::settings::Settings;

const FACTORY_FAC: &[&str] = &[
    include_str!("../presets-fx/1.fac"),
    include_str!("../presets-fx/2.fac"),
    include_str!("../presets-fx/3.fac"),
    include_str!("../presets-fx/4.fac"),
    include_str!("../presets-fx/5.fac"),
    include_str!("../presets-fx/6.fac"),
    include_str!("../presets-fx/7.fac"),
    include_str!("../presets-fx/8.fac"),
    include_str!("../presets-fx/9.fac"),
    include_str!("../presets-fx/10.fac"),
    include_str!("../presets-fx/11.fac"),
    include_str!("../presets-fx/12.fac"),
];

/// All built-in presets that parse cleanly (name + converted settings).
pub fn factory_presets() -> Vec<(String, Settings)> {
    FACTORY_FAC
        .iter()
        .filter_map(|text| match parse_fac(text) {
            Ok(p) => Some((p.name.clone().to_lowercase(), p.to_settings())),
            Err(e) => {
                log::warn!("factory preset parse failed: {e}");
                None
            }
        })
        .collect()
}

/// Case-insensitive builtin lookup; user presets must be checked first by the caller.
pub fn load(name: &str) -> Option<Settings> {
    let key = name.to_lowercase();
    factory_presets()
        .into_iter()
        .find(|(n, _)| *n == key)
        .map(|(_, s)| s)
}

/// Lowercased display names of every built-in preset, in file order.
pub fn names() -> Vec<String> {
    FACTORY_FAC
        .iter()
        .filter_map(|text| parse_fac(text).ok().map(|p| p.name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_twelve_factory_presets_load() {
        let presets = factory_presets();
        assert_eq!(presets.len(), 12);
        let names = names();
        assert_eq!(
            names,
            vec![
                "General",
                "Music",
                "Voice",
                "Volume Boost",
                "Gaming",
                "Classic Processing",
                "Light Processing",
                "Bass Boost",
                "Streaming Video",
                "Movies",
                "TV",
                "Transcription",
            ]
        );
        for (_, s) in &presets {
            assert!(s.clarity.amount <= 100.0 && s.bass_boost.amount <= 100.0);
            assert_eq!(s.multi_bands.len(), 10);
            assert!(s.limiter);
            assert!(!s.bypass);
        }
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert!(load("volume boost").is_some());
        assert!(load("VOLUME BOOST").is_some());
        assert!(load("no such preset").is_none());
    }
}
