//! anonctl — command-line control for the anontokyo daemon.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use anontokyo::ctl::{self, Request, Response};
use anontokyo::settings::{BoostKind, EqMode, Settings};

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

/// low/mid/high = simple-mode bands, m<N> = multi-band index
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
            "mid" => Ok(BandArg {
                mode: EqMode::Simple,
                index: 1,
            }),
            "high" => Ok(BandArg {
                mode: EqMode::Simple,
                index: 2,
            }),
            other => {
                if let Some(rest) = other.strip_prefix('m') {
                    let idx: usize = rest
                        .parse()
                        .map_err(|_| format!("bad band '{s}' (use low|mid|high|m<index>)"))?;
                    Ok(BandArg {
                        mode: EqMode::Multi,
                        index: idx,
                    })
                } else {
                    Err(format!("bad band '{s}' (use low|mid|high|m<index>)"))
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
    /// stop the daemon (restores the previous default sink)
    Stop,
    /// send a raw JSON request (escape hatch)
    Raw { json: String },
}

#[derive(Subcommand)]
enum EqCmd {
    /// switch EQ mode: simple (3 band) or multi (N band)
    Mode { mode: EqModeArg },
    /// set a band: anonctl eq set low|mid|high|m<index> --gain 3 --freq 900 --q 1.5
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
        EqMode::Simple => format!(
            "simple({:+.1}/{:+.1}/{:+.1})",
            s.simple_bands[0].gain_db, s.simple_bands[1].gain_db, s.simple_bands[2].gain_db
        ),
        EqMode::Multi => {
            let active = s
                .multi_bands
                .iter()
                .filter(|b| b.gain_db.abs() > 0.01)
                .count();
            format!("multi({} bands, {} touched)", s.multi_bands.len(), active)
        }
    };
    format!(
        "bypass={} preamp={:+.1}dB limiter={} {} {} {} eq={} ",
        if s.bypass { "on" } else { "off" },
        s.preamp_db,
        if s.limiter { "on" } else { "off" },
        boost("bass", &s.bass),
        boost("vocal", &s.vocal),
        boost("treble", &s.treble),
        eq,
    )
    .trim()
    .to_string()
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
        println!("{}", serde_json::to_string_pretty(status)?);
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
        },
        Cmd::Presets => Request::ListPresets,
        Cmd::Stop => Request::Stop,
        Cmd::Raw { json } => {
            let req: Request =
                serde_json::from_str(json).map_err(|e| anyhow::anyhow!("bad request JSON: {e}"))?;
            req
        }
    };
    let resp = ctl::request(&req)?;
    report(&cli, resp)
}
