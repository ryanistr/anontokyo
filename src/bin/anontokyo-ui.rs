//! M3-styled Slint control panel; thin client over the daemon socket.

use anontokyo::ctl::{self, Request, Response};
use anontokyo::meter;
use anontokyo::settings::{BoostKind, EqMode, FxKind};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

slint::include_modules!();

type PresetNames = Rc<RefCell<Vec<String>>>;

fn send(req: Request) -> Option<Response> {
    match ctl::request(&req) {
        Ok(r) if r.ok => Some(r),
        _ => None,
    }
}

fn fmt_freq(hz: f64) -> SharedString {
    if hz >= 1000.0 {
        format!("{:.1}k", hz / 1000.0).into()
    } else {
        format!("{:.0}Hz", hz).into()
    }
}

/// Pull a status snapshot from the daemon into the UI. Returns false when the
/// daemon is unreachable.
fn refresh(ui: &MainWindow, preset_names: &PresetNames) -> bool {
    let resp = match ctl::request(&Request::Status) {
        Ok(r) if r.ok => r,
        _ => {
            ui.set_status_text("daemon not running — retrying...".into());
            return false;
        }
    };
    let status = resp.status.unwrap_or(serde_json::Value::Null);
    let s = &status["settings"];

    ui.set_loading(true);
    ui.set_bypass(s["bypass"].as_bool().unwrap_or(false));
    ui.set_limiter(s["limiter"].as_bool().unwrap_or(false));
    ui.set_preamp_db(s["preamp_db"].as_f64().unwrap_or(0.0) as f32);

    ui.set_boost_bass(s["bass"]["enabled"].as_bool().unwrap_or(false));
    ui.set_boost_vocal(s["vocal"]["enabled"].as_bool().unwrap_or(false));
    ui.set_boost_treble(s["treble"]["enabled"].as_bool().unwrap_or(false));

    for (key, idx) in [
        ("clarity", 0u8),
        ("surround", 1),
        ("ambience", 2),
        ("dynamic_boost", 3),
        ("bass_boost", 4),
    ] {
        let on = s[key]["enabled"].as_bool().unwrap_or(false);
        let amount = s[key]["amount"].as_f64().unwrap_or(0.0) as f32;
        match idx {
            0 => {
                ui.set_fx_clarity_on(on);
                ui.set_fx_clarity(amount);
            }
            1 => {
                ui.set_fx_surround_on(on);
                ui.set_fx_surround(amount);
            }
            2 => {
                ui.set_fx_ambience_on(on);
                ui.set_fx_ambience(amount);
            }
            3 => {
                ui.set_fx_dynamic_on(on);
                ui.set_fx_dynamic(amount);
            }
            _ => {
                ui.set_fx_bassboost_on(on);
                ui.set_fx_bassboost(amount);
            }
        }
    }

    let multi = s["eq_mode"].as_str() == Some("multi");
    ui.set_eq_mode(i32::from(multi));
    let bands = if multi {
        &s["multi_bands"]
    } else {
        &s["simple_bands"]
    };
    let empty = Vec::new();
    let arr = bands.as_array().unwrap_or(&empty);
    let gains: Vec<f32> = arr
        .iter()
        .map(|b| b["gain_db"].as_f64().unwrap_or(0.0) as f32)
        .collect();
    let labels: Vec<SharedString> = arr
        .iter()
        .map(|b| fmt_freq(b["freq_hz"].as_f64().unwrap_or(0.0)))
        .collect();
    ui.set_band_gains(ModelRc::new(VecModel::from(gains)));
    ui.set_band_labels(ModelRc::new(VecModel::from(labels)));

    let presets: Vec<String> = resp.presets.unwrap_or_default();
    *preset_names.borrow_mut() = presets.clone();
    let items: Vec<MenuItem> = presets
        .iter()
        .map(|p| MenuItem {
            text: p.as_str().into(),
            enabled: true,
            ..Default::default()
        })
        .collect();
    ui.set_preset_items(ModelRc::new(VecModel::from(items)));

    let rate = status["format"]["rate"].as_u64().unwrap_or(0);
    let stages = status["active_stages"].as_u64().unwrap_or(0);
    ui.set_status_text(format!("live · {rate} Hz · {stages} stages active").into());
    ui.set_loading(false);
    true
}

fn fx_kind(idx: i32) -> FxKind {
    match idx {
        0 => FxKind::Clarity,
        1 => FxKind::Surround,
        2 => FxKind::Ambience,
        3 => FxKind::DynamicBoost,
        _ => FxKind::BassBoost,
    }
}

fn boost_kind(idx: i32) -> BoostKind {
    match idx {
        0 => BoostKind::Bass,
        1 => BoostKind::Vocal,
        _ => BoostKind::Treble,
    }
}

fn main() -> Result<(), slint::PlatformError> {
    env_logger::init();
    let ui = MainWindow::new()?;
    if std::env::args().any(|a| a == "--advanced") {
        ui.set_open_advanced(true);
    }

    let preset_names: PresetNames = Rc::new(RefCell::new(Vec::new()));
    let names = preset_names.clone();
    let loaded_preset: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    {
        let weak = ui.as_weak();
        let names = names.clone();
        let loaded = loaded_preset.clone();
        ui.on_preset_selected(move |index| {
            let name = names.borrow().get(index as usize).cloned();
            if let Some(name) = name {
                if send(Request::LoadPreset { name: name.clone() }).is_some() {
                    *loaded.borrow_mut() = Some(name);
                }
            }
            if let Some(ui) = weak.upgrade() {
                refresh(&ui, &names);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let names = names.clone();
        let loaded = loaded_preset.clone();
        ui.on_reload_clicked(move || {
            if let Some(name) = loaded.borrow().clone() {
                send(Request::LoadPreset { name });
            }
            if let Some(ui) = weak.upgrade() {
                refresh(&ui, &names);
            }
        });
    }

    macro_rules! simple_callback {
        ($setter:ident, |$ui:ident $(, $arg:pat_param)*| $body:block) => {{
            let weak = ui.as_weak();
            let names = names.clone();
            ui.$setter(move |$($arg),*| {
                if let Some($ui) = weak.upgrade() {
                    $body
                    refresh(&$ui, &names);
                }
            });
        }};
    }

    simple_callback!(on_set_bypass, |_ui, enabled| {
        send(Request::SetBypass { enabled });
    });
    simple_callback!(on_set_limiter, |_ui, enabled| {
        send(Request::SetLimiter { enabled });
    });
    simple_callback!(on_set_preamp, |_ui, db| {
        send(Request::SetPreamp { db });
    });
    simple_callback!(on_set_boost, |_ui, which, enabled| {
        send(Request::SetBoost {
            which: boost_kind(which),
            enabled: Some(enabled),
            gain_db: None,
            freq_hz: None,
            q: None,
        });
    });
    simple_callback!(on_set_fx, |_ui, which, enabled, amount| {
        send(Request::SetFx {
            which: fx_kind(which),
            enabled: Some(enabled),
            amount: Some(amount),
        });
    });
    simple_callback!(on_set_eq_mode, |_ui, index| {
        send(Request::SetEqMode {
            mode: if index == 1 {
                EqMode::Multi
            } else {
                EqMode::Simple
            },
        });
    });
    simple_callback!(on_set_band_gain, |ui, index, gain| {
        let mode = if ui.get_eq_mode() == 1 {
            EqMode::Multi
        } else {
            EqMode::Simple
        };
        send(Request::SetBand {
            mode,
            index: index as usize,
            freq_hz: None,
            gain_db: Some(gain),
            q: None,
        });
    });

    refresh(&ui, &names);

    // spectrum band labels: 24 log-spaced columns shared with the bars
    let meter_labels: Vec<SharedString> = meter::band_freqs()
        .iter()
        .map(|&f| {
            if f >= 1000.0 {
                SharedString::from(format!("{:.1}k", f / 1000.0))
            } else {
                SharedString::from(format!("{:.0}", f))
            }
        })
        .collect();
    ui.set_meter_labels(ModelRc::new(VecModel::from(meter_labels)));

    // keep the panel in sync with daemon-side changes (other controllers)
    let timer = Timer::default();
    let weak = ui.as_weak();
    timer.start(
        TimerMode::Repeated,
        std::time::Duration::from_millis(500),
        move || {
            if let Some(ui) = weak.upgrade() {
                refresh(&ui, &names);
            }
        },
    );

    // fast path for the visualizer; Meter requests stay at debug log level
    let meter_timer = Timer::default();
    let meter_weak = ui.as_weak();
    meter_timer.start(
        TimerMode::Repeated,
        std::time::Duration::from_millis(60),
        move || {
            let Some(ui) = meter_weak.upgrade() else {
                return;
            };
            let Ok(resp) = ctl::request(&Request::Meter) else {
                return;
            };
            if !resp.ok {
                return;
            }
            let Some(bands) = resp.status.as_ref().and_then(|s| s["bands"].as_array()) else {
                return;
            };
            let levels: Vec<f32> = bands
                .iter()
                .filter_map(|v| v.as_f64())
                .map(|v| v as f32)
                .collect();
            if levels.len() == meter::METER_BANDS {
                let avg = |s: &[f32]| s.iter().sum::<f32>() / s.len() as f32;
                let logo = vec![
                    avg(&levels[0..4]),
                    avg(&levels[10..14]),
                    avg(&levels[20..24]),
                ];
                ui.set_logo_bars(ModelRc::new(VecModel::from(logo)));
            }
            ui.set_meter_levels(ModelRc::new(VecModel::from(levels)));
        },
    );

    ui.run()
}
