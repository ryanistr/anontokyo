//! anonctl — command-line control for the anontokyo daemon.

use anontokyo::ctl::{self, Request, Response};
use anontokyo::settings::{Band, Boost, BoostKind, EqMode, Fx, FxKind, Settings};
use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "anonctl", about = "Control the AnonTokyo audio daemon")]
struct Cli {
    /// print the raw JSON response
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(ValueEnum, Clone, Copy)]
enum OnOff {
    On,
    Off,
}

impl From<OnOff> for bool {
    fn from(v: OnOff) -> bool {
        matches!(v, OnOff::On)
    }
}

#[derive(ValueEnum, Clone, Copy)]
enum BoostArg {
    Bass,
    Vocal,
    Treble,
}

impl From<BoostArg> for BoostKind {
    fn from(v: BoostArg) -> BoostKind {
        match v {
            BoostArg::Bass => BoostKind::Bass,
            BoostArg::Vocal => BoostKind::Vocal,
            BoostArg::Treble => BoostKind::Treble,
        }
    }
}

#[derive(ValueEnum, Clone, Copy)]
enum FxArg {
    Clarity,
    Surround,
    Ambience,
    DynamicBoost,
    BassBoost,
}

impl From<FxArg> for FxKind {
    fn from(v: FxArg) -> FxKind {
        match v {
            FxArg::Clarity => FxKind::Clarity,
            FxArg::Surround => FxKind::Surround,
            FxArg::Ambience => FxKind::Ambience,
            FxArg::DynamicBoost => FxKind::DynamicBoost,
            FxArg::BassBoost => FxKind::BassBoost,
        }
    }
}

/// low..high = 5 simple bands, s<N> = simple index, m<N> = multi index
#[derive(Clone)]
struct BandArg {
    mode: EqMode,
    index: usize,
}

impl std::str::FromStr for BandArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Ok(BandArg {
                mode: EqMode::Simple,
                index: 0,
            }),
            "lowmid" => Ok(BandArg {
                mode: EqMode::Simple,
                index: 1,
            }),
            "mid" => Ok(BandArg {
                mode: EqMode::Simple,
                index: 2,
            }),
            "highmid" => Ok(BandArg {
                mode: EqMode::Simple,
                index: 3,
            }),
            "high" => Ok(BandArg {
                mode: EqMode::Simple,
                index: 4,
            }),
            other => {
                let usage = "use low|lowmid|mid|highmid|high|s<index>|m<index>";
                if let Some(rest) = other.strip_prefix('m') {
                    let idx: usize = rest
                        .parse()
                        .map_err(|_| format!("bad band '{s}' ({usage})"))?;
                    Ok(BandArg {
                        mode: EqMode::Multi,
                        index: idx,
                    })
                } else if let Some(rest) = other.strip_prefix('s') {
                    let idx: usize = rest
                        .parse()
                        .map_err(|_| format!("bad band '{s}' ({usage})"))?;
                    Ok(BandArg {
                        mode: EqMode::Simple,
                        index: idx,
                    })
                } else {
                    Err(format!("bad band '{s}' ({usage})"))
                }
            }
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// show daemon status and current settings
    Status,
    /// master bypass: audio passes through unprocessed
    Bypass { state: OnOff },
    /// soft output limiter (keeps boosts from clipping)
    Limiter { state: OnOff },
    /// preamp gain in dB, applied before boosts/EQ
    #[command(allow_negative_numbers = true)]
    Preamp { db: f32 },
    /// change one of the three boosts
    Boost {
        which: BoostArg,
        /// enable this boost
        #[arg(long)]
        enable: bool,
        /// disable this boost
        #[arg(long)]
        disable: bool,
        /// gain in dB (-12..12)
        #[arg(long, allow_hyphen_values = true)]
        gain: Option<f32>,
        /// corner/center frequency in Hz
        #[arg(long, allow_hyphen_values = true)]
        freq: Option<f32>,
        /// Q (vocal boost only; ignored by shelves)
        #[arg(long, allow_hyphen_values = true)]
        q: Option<f32>,
    },
    /// change one of the five FxSound-style slider effects
    Fx {
        which: FxArg,
        /// enable this effect
        #[arg(long)]
        enable: bool,
        /// disable this effect
        #[arg(long)]
        disable: bool,
        /// slider amount 0..100
        #[arg(long)]
        amount: Option<f32>,
    },
    /// EQ section
    Eq {
        #[command(subcommand)]
        cmd: EqCmd,
    },
    /// preset management
    Preset {
        #[command(subcommand)]
        cmd: PresetCmd,
    },
    /// list saved presets
    Presets,
    /// print the control schema (for building a GUI)
    Describe,
    /// stop the daemon (restores the previous default sink)
    Stop,
    /// send a raw JSON request (escape hatch)
    Raw { payload: String },
}

#[derive(Subcommand)]
enum EqCmd {
    /// switch EQ mode: simple (5 band) or multi (N band)
    Mode { mode: EqModeArg },
    /// set a band: anonctl eq set low|lowmid|mid|highmid|high|m<index> --gain 3 --freq 900 --q 1.5
    #[command(allow_negative_numbers = true)]
    Set {
        band: BandArg,
        #[arg(long, allow_hyphen_values = true)]
        gain: Option<f32>,
        #[arg(long, allow_hyphen_values = true)]
        freq: Option<f32>,
        #[arg(long, allow_hyphen_values = true)]
        q: Option<f32>,
    },
    /// set the number of bands in multi mode (1..32)
    Count { count: usize },
    /// show the EQ section only
    Show,
}

#[derive(ValueEnum, Clone, Copy)]
enum EqModeArg {
    Simple,
    Multi,
}

#[derive(Subcommand)]
enum PresetCmd {
    /// save current settings as a preset
    Save { name: String },
    /// load a preset (applied + persisted as the active config)
    Load { name: String },
    /// delete a saved preset (built-in presets are protected)
    Delete { name: String },
    /// rename a saved preset
    Rename { from: String, to: String },
    /// import a FxSound .fac preset file as a user preset
    Import { path: String },
}

fn summarize(s: &Settings) -> String {
    let boost = |name: &str, b: &anontokyo::settings::Boost| -> String {
        if b.enabled {
            format!("{name}[{:+.1}dB@{:.0}Hz]", b.gain_db, b.freq_hz)
        } else {
            format!("{name}[off]")
        }
    };
    let eq = match s.eq_mode {
        EqMode::Simple => {
            let gains: Vec<String> = s
                .simple_bands
                .iter()
                .map(|b| format!("{:+.1}", b.gain_db))
                .collect();
            format!("simple({})", gains.join("/"))
        }
        EqMode::Multi => {
            let active = s
                .multi_bands
                .iter()
                .filter(|b| b.gain_db.abs() > 0.01)
                .count();
            format!("multi({} bands, {} touched)", s.multi_bands.len(), active)
        }
    };
    let mut fx = Vec::new();
    for (name, e) in [
        ("clr", &s.clarity),
        ("sur", &s.surround),
        ("amb", &s.ambience),
        ("dyn", &s.dynamic_boost),
        ("bass", &s.bass_boost),
    ] {
        if e.enabled {
            fx.push(format!("{name}{}", e.amount.round()));
        }
    }
    let fx_part = if fx.is_empty() {
        String::new()
    } else {
        format!(" fx({})", fx.join(" "))
    };
    format!(
        "bypass={} preamp={:+.1}dB limiter={} {} {} {} eq={}{} ",
        if s.bypass { "on" } else { "off" },
        s.preamp_db,
        if s.limiter { "on" } else { "off" },
        boost("bass", &s.bass),
        boost("vocal", &s.vocal),
        boost("treble", &s.treble),
        eq,
        fx_part,
    )
    .trim()
    .to_string()
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn fmt_hz(v: f32) -> String {
    if (v - v.round()).abs() > 0.01 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

fn fmt_boost(b: &Boost) -> String {
    if b.enabled {
        format!("{:+.1} dB @{} Hz", b.gain_db, fmt_hz(b.freq_hz))
    } else {
        "off".into()
    }
}

fn fmt_fx(f: &Fx) -> String {
    match (f.enabled, f.amount) {
        (false, _) => "off".into(),
        (true, a) if a <= 0.0 => "on".into(),
        (true, a) => format!("{}%", a.round() as i32),
    }
}

/// Human-friendly status block; `--json` still prints the raw payload.
fn human_status(status: &serde_json::Value, presets: Option<&[String]>) -> String {
    let settings: Settings = status
        .get("settings")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let rate = status["format"]["rate"].as_u64().unwrap_or(0);
    let ch = status["format"]["channels"].as_u64().unwrap_or(0);
    let stages = status["active_stages"].as_u64().unwrap_or(0);
    let word = if stages == 1 { "stage" } else { "stages" };

    let mut out = format!("anontokyo · {rate} Hz · {ch} ch · {stages} {word} active");
    if settings.bypass {
        out.push_str(" · BYPASSED");
    }
    out.push_str(&format!(
        "\n{:<8}bypass {} · preamp {:+.1} dB · limiter {}",
        "master",
        on_off(settings.bypass),
        settings.preamp_db,
        on_off(settings.limiter)
    ));
    out.push_str(&format!(
        "\n{:<8}bass {} · vocal {} · treble {}",
        "boosts",
        fmt_boost(&settings.bass),
        fmt_boost(&settings.vocal),
        fmt_boost(&settings.treble)
    ));
    out.push_str(&format!(
        "\n{:<8}clarity {} · surround {} · ambience {} · dynamic {} · bass {}",
        "fx",
        fmt_fx(&settings.clarity),
        fmt_fx(&settings.surround),
        fmt_fx(&settings.ambience),
        fmt_fx(&settings.dynamic_boost),
        fmt_fx(&settings.bass_boost)
    ));
    let eq = match settings.eq_mode {
        EqMode::Simple => {
            let sb = &settings.simple_bands;
            if sb.is_empty() {
                "simple".into()
            } else {
                let parts: Vec<String> = sb
                    .iter()
                    .map(|b| format!("{:+.1} @{} Hz", b.gain_db, fmt_hz(b.freq_hz)))
                    .collect();
                format!("simple · {}", parts.join(" · "))
            }
        }
        EqMode::Multi => {
            let touched: Vec<(usize, &Band)> = settings
                .multi_bands
                .iter()
                .enumerate()
                .filter(|(_, b)| b.gain_db.abs() > 0.01)
                .collect();
            if touched.is_empty() {
                format!("multi · {} bands · flat", settings.multi_bands.len())
            } else {
                let shown: Vec<String> = touched
                    .iter()
                    .take(6)
                    .map(|(i, b)| format!("m{i} {:+.1} @{} Hz", b.gain_db, fmt_hz(b.freq_hz)))
                    .collect();
                let more = if touched.len() > 6 { " …" } else { "" };
                format!(
                    "multi · {} bands · {} touched: {}{}",
                    settings.multi_bands.len(),
                    touched.len(),
                    shown.join(" · "),
                    more
                )
            }
        }
    };
    out.push_str(&format!("\n{:<8}{}", "eq", eq));
    if let Some(p) = presets {
        out.push_str(&format!("\n{:<8}{}", "presets", p.len()));
    }
    out
}

fn report(cli: &Cli, resp: Response) -> Result<()> {
    if !resp.ok {
        bail!(resp.error.unwrap_or_else(|| "unknown daemon error".into()));
    }
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    if let Some(status) = &resp.status {
        if status.get("settings").is_some() {
            println!("{}", human_status(status, resp.presets.as_deref()));
        } else {
            // describe/schema payloads stay raw JSON
            println!("{}", serde_json::to_string_pretty(status)?);
        }
        return Ok(());
    }
    if let Some(settings) = &resp.settings {
        println!("{}", summarize(settings));
        return Ok(());
    }
    if let Some(presets) = &resp.presets {
        if presets.is_empty() {
            println!("(no presets)");
        } else {
            for p in presets {
                println!("{p}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(settings: &Settings) -> serde_json::Value {
        serde_json::json!({
            "settings": settings,
            "format": {"rate": 48000, "channels": 2},
            "active_stages": 3,
        })
    }

    #[test]
    fn human_status_shows_every_section() {
        let out = human_status(&payload(&Settings::default()), Some(&["General".into()]));
        assert!(out.contains("48000 Hz · 2 ch · 3 stages active"), "{out}");
        assert!(out.contains("master"), "{out}");
        assert!(out.contains("boosts"), "{out}");
        assert!(out.contains("fx"), "{out}");
        assert!(out.contains("eq"), "{out}");
        assert!(out.contains("presets 1"), "{out}");
    }

    #[test]
    fn human_status_renders_fx_amounts_and_multi_eq() {
        let mut s = Settings::default();
        s.clarity = Fx::new(true, 25.0);
        s.bass_boost = Fx::new(true, 0.0); // enabled at zero reads as "on"
        s.eq_mode = EqMode::Multi;
        s.multi_bands[0].gain_db = 2.0;
        s.multi_bands[3].gain_db = -1.0;
        let out = human_status(&payload(&s), None);
        assert!(out.contains("clarity 25%"), "{out}");
        assert!(out.contains("surround off"), "{out}");
        assert!(out.contains("bass on"), "{out}");
        assert!(out.contains("2 touched"), "{out}");
        assert!(out.contains("m0 +2.0"), "{out}");
        assert!(!out.contains("presets"), "{out}");
    }

    #[test]
    fn bypass_marks_header() {
        let mut s = Settings::default();
        s.bypass = true;
        let out = human_status(&payload(&s), None);
        assert!(out.contains("BYPASSED"), "{out}");
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let req = match &cli.cmd {
        Cmd::Status => Request::Status,
        Cmd::Bypass { state } => Request::SetBypass {
            enabled: (*state).into(),
        },
        Cmd::Limiter { state } => Request::SetLimiter {
            enabled: (*state).into(),
        },
        Cmd::Preamp { db } => Request::SetPreamp { db: *db },
        Cmd::Boost {
            which,
            enable,
            disable,
            gain,
            freq,
            q,
        } => {
            if *enable == *disable && gain.is_none() && freq.is_none() && q.is_none() {
                bail!("give one of --enable/--disable/--gain/--freq/--q");
            }
            Request::SetBoost {
                which: (*which).into(),
                enabled: if *enable {
                    Some(true)
                } else if *disable {
                    Some(false)
                } else {
                    None
                },
                gain_db: *gain,
                freq_hz: *freq,
                q: *q,
            }
        }
        Cmd::Fx {
            which,
            enable,
            disable,
            amount,
        } => {
            if *enable == *disable && amount.is_none() {
                bail!("give one of --enable/--disable/--amount");
            }
            Request::SetFx {
                which: (*which).into(),
                enabled: if *enable {
                    Some(true)
                } else if *disable {
                    Some(false)
                } else {
                    None
                },
                amount: *amount,
            }
        }
        Cmd::Eq { cmd } => match cmd {
            EqCmd::Mode { mode } => Request::SetEqMode {
                mode: match mode {
                    EqModeArg::Simple => EqMode::Simple,
                    EqModeArg::Multi => EqMode::Multi,
                },
            },
            EqCmd::Set {
                band,
                gain,
                freq,
                q,
            } => {
                if gain.is_none() && freq.is_none() && q.is_none() {
                    bail!("give at least one of --gain/--freq/--q");
                }
                Request::SetBand {
                    mode: band.mode,
                    index: band.index,
                    freq_hz: *freq,
                    gain_db: *gain,
                    q: *q,
                }
            }
            EqCmd::Count { count } => Request::SetBandCount { count: *count },
            EqCmd::Show => Request::Status,
        },
        Cmd::Preset { cmd } => match cmd {
            PresetCmd::Save { name } => Request::SavePreset { name: name.clone() },
            PresetCmd::Load { name } => Request::LoadPreset { name: name.clone() },
            PresetCmd::Delete { name } => Request::DeletePreset { name: name.clone() },
            PresetCmd::Rename { from, to } => Request::RenamePreset {
                from: from.clone(),
                to: to.clone(),
            },
            PresetCmd::Import { path } => Request::ImportPreset { path: path.clone() },
        },
        Cmd::Presets => Request::ListPresets,
        Cmd::Describe => Request::Describe,
        Cmd::Stop => Request::Stop,
        Cmd::Raw { payload } => {
            let req: Request = serde_json::from_str(payload)
                .map_err(|e| anyhow::anyhow!("bad request JSON: {e}"))?;
            req
        }
    };
    let resp = ctl::request(&req)?;
    report(&cli, resp)
}
