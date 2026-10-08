//! Control plane: shared state, JSON-line protocol over a unix socket,
//! server thread and client helper.

use crate::chain::ChainDef;
use crate::settings::{self, BoostKind, EqMode, Settings};
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub rate: u32,
    pub channels: usize,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self {
            rate: 48_000,
            channels: 2,
        }
    }
}

/// Everything the daemon and the control thread share.
pub struct Shared {
    pub settings: ArcSwap<Settings>,
    pub format: ArcSwap<AudioFormat>,
    pub chain: ArcSwap<ChainDef>,
    rebuild_lock: Mutex<()>,
    config_path: PathBuf,
}

impl Shared {
    pub fn load(config_path: PathBuf) -> Arc<Self> {
        let settings = settings::load_settings(&config_path);
        let format = AudioFormat::default();
        let chain = ChainDef::new(&settings, format.rate, format.channels);
        Arc::new(Self {
            settings: arc_swap::ArcSwap::from_pointee(settings),
            format: arc_swap::ArcSwap::from_pointee(format),
            chain: arc_swap::ArcSwap::from_pointee(chain),
            rebuild_lock: Mutex::new(()),
            config_path,
        })
    }

    /// Mutate settings, persist to disk, rebuild the chain. Returns the new
    /// settings, or an error message if the mutation/save failed.
    pub fn update<F>(&self, f: F) -> Result<Settings, String>
    where
        F: FnOnce(&mut Settings) -> Result<(), String>,
    {
        let mut new = (*self.settings.load_full()).clone();
        f(&mut new)?;
        new.sanitize();
        settings::save_settings(&self.config_path, &new)?;
        let _guard = self.rebuild_lock.lock().map_err(|e| e.to_string())?;
        let format = **self.format.load();
        let chain = ChainDef::new(&new, format.rate, format.channels);
        self.chain.store(Arc::new(chain));
        self.settings.store(Arc::new(new.clone()));
        Ok(new)
    }

    /// Replace settings wholesale (preset load). Persists like `update`.
    pub fn replace(&self, new: Settings) -> Result<Settings, String> {
        self.update(|slot| {
            *slot = new;
            Ok(())
        })
    }

    /// Called from the PipeWire thread when the negotiated audio format changes.
    pub fn set_format(&self, format: AudioFormat) {
        let _guard = self.rebuild_lock.lock().ok();
        let current = **self.format.load();
        if current == format {
            return;
        }
        self.format.store(Arc::new(format));
        let settings = self.settings.load_full();
        let chain = ChainDef::new(&settings, format.rate, format.channels);
        self.chain.store(Arc::new(chain));
    }

    pub fn snapshot(&self) -> Settings {
        (*self.settings.load_full()).clone()
    }

    pub fn status_line(&self) -> serde_json::Value {
        let s = self.snapshot();
        let f = **self.format.load();
        serde_json::json!({
            "settings": s,
            "format": {"rate": f.rate, "channels": f.channels},
            "active_stages": self.chain.load().stages.len(),
        })
    }
}

// ---------- protocol ----------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    SetBypass {
        enabled: bool,
    },
    SetPreamp {
        db: f32,
    },
    SetLimiter {
        enabled: bool,
    },
    SetBoost {
        which: BoostKind,
        enabled: Option<bool>,
        gain_db: Option<f32>,
        freq_hz: Option<f32>,
        q: Option<f32>,
    },
    SetEqMode {
        mode: EqMode,
    },
    SetBand {
        mode: EqMode,
        index: usize,
        freq_hz: Option<f32>,
        gain_db: Option<f32>,
        q: Option<f32>,
    },
    SetBandCount {
        count: usize,
    },
    SavePreset {
        name: String,
    },
    LoadPreset {
        name: String,
    },
    ListPresets,
    Stop,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<Settings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presets: Option<Vec<String>>,
}

impl Response {
    fn ok_settings(settings: Settings) -> Self {
        Self {
            ok: true,
            error: None,
            settings: Some(settings),
            status: None,
            presets: None,
        }
    }
    fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            settings: None,
            status: None,
            presets: None,
        }
    }
}

fn handle(shared: &Shared, req: Request) -> (Response, bool) {
    let mut stop = false;
    let resp = match req {
        Request::Status => Response {
            ok: true,
            error: None,
            settings: None,
            status: Some(shared.status_line()),
            presets: settings::list_presets().ok(),
        },
        Request::SetBypass { enabled } => shared
            .update(|s| {
                s.bypass = enabled;
                Ok(())
            })
            .map(Response::ok_settings)
            .unwrap_or_else(Response::err),
        Request::SetPreamp { db } => shared
            .update(|s| {
                s.preamp_db = db;
                Ok(())
            })
            .map(Response::ok_settings)
            .unwrap_or_else(Response::err),
        Request::SetLimiter { enabled } => shared
            .update(|s| {
                s.limiter = enabled;
                Ok(())
            })
            .map(Response::ok_settings)
            .unwrap_or_else(Response::err),
        Request::SetBoost {
            which,
            enabled,
            gain_db,
            freq_hz,
            q,
        } => shared
            .update(|s| {
                let b = s.boost_mut(which);
                if let Some(v) = enabled {
                    b.enabled = v;
                }
                if let Some(v) = gain_db {
                    b.gain_db = v;
                }
                if let Some(v) = freq_hz {
                    b.freq_hz = v;
                }
                if let Some(v) = q {
                    b.q = v;
                }
                Ok(())
            })
            .map(Response::ok_settings)
            .unwrap_or_else(Response::err),
        Request::SetEqMode { mode } => shared
            .update(|s| {
                s.eq_mode = mode;
                Ok(())
            })
            .map(Response::ok_settings)
            .unwrap_or_else(Response::err),
        Request::SetBand {
            mode,
            index,
            freq_hz,
            gain_db,
            q,
        } => shared
            .update(|s| {
                let Some(band) = s.band(mode, index).cloned() else {
                    return Err(format!("no band {index} in {:?} mode", mode));
                };
                let mut band = band;
                if let Some(v) = freq_hz {
                    band.freq_hz = v;
                }
                if let Some(v) = gain_db {
                    band.gain_db = v;
                }
                if let Some(v) = q {
                    band.q = v;
                }
                if !s.set_band(mode, index, band) {
                    return Err(format!("no band {index}"));
                }
                Ok(())
            })
            .map(Response::ok_settings)
            .unwrap_or_else(Response::err),
        Request::SetBandCount { count } => shared
            .update(|s| {
                s.set_band_count(count);
                Ok(())
            })
            .map(Response::ok_settings)
            .unwrap_or_else(Response::err),
        Request::SavePreset { name } => match settings::save_preset(&name, &shared.snapshot()) {
            Ok(()) => Response::ok_settings(shared.snapshot()),
            Err(e) => Response::err(e),
        },
        Request::LoadPreset { name } => match settings::load_preset(&name) {
            Ok(preset) => shared
                .replace(preset)
                .map(Response::ok_settings)
                .unwrap_or_else(Response::err),
            Err(e) => Response::err(e),
        },
        Request::ListPresets => match settings::list_presets() {
            Ok(presets) => Response {
                ok: true,
                error: None,
                settings: None,
                status: None,
                presets: Some(presets),
            },
            Err(e) => Response::err(e),
        },
        Request::Stop => {
            stop = true;
            Response::ok_settings(shared.snapshot())
        }
    };
    (resp, stop)
}

pub fn socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    match runtime {
        Some(dir) => dir.join("anontokyo.sock"),
        None => settings::config_dir().join("anontokyo.sock"),
    }
}

/// Serve control requests until `should_stop` returns true.
pub fn serve(
    shared: Arc<Shared>,
    should_stop: impl Fn() -> bool + Send + Sync + 'static,
    on_stop: impl Fn() + Send + Sync + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let path = socket_path();
    if path.exists() {
        // stale socket from a crashed daemon: only remove if nothing is answering
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("another daemon is already listening on {path:?}"),
            ));
        }
        std::fs::remove_file(&path)?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    log::info!("control socket: {path:?}");

    let handle = std::thread::Builder::new()
        .name("anontokyo-ctl".into())
        .spawn(move || {
            while !should_stop() {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        use std::io::{BufRead, Write};
                        stream
                            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                            .ok();
                        stream
                            .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                            .ok();
                        let mut line = String::new();
                        let Ok(cloned) = stream.try_clone() else {
                            continue;
                        };
                        let mut reader = std::io::BufReader::new(cloned);
                        let read = reader.read_line(&mut line);
                        let (resp, stop) = match read {
                            Ok(0) => continue,
                            Ok(_) => match serde_json::from_str::<Request>(line.trim()) {
                                Ok(req) => handle(&shared, req),
                                Err(e) => (Response::err(format!("bad request: {e}")), false),
                            },
                            Err(_) => continue,
                        };
                        let mut out = serde_json::to_string(&resp).unwrap_or_default();
                        out.push('\n');
                        let _ = stream.write_all(out.as_bytes());
                        let _ = stream.flush();
                        if stop {
                            on_stop();
                            break;
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(e) => {
                        log::error!("control socket accept failed: {e}");
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                }
            }
            std::fs::remove_file(&path).ok();
            log::info!("control socket closed");
        })?;
    Ok(handle)
}

/// One-shot client: send a request, get the response.
pub fn request(req: &Request) -> anyhow::Result<Response> {
    use std::io::{BufRead, Write};
    let path = socket_path();
    let stream = std::os::unix::net::UnixStream::connect(&path)
        .map_err(|e| anyhow::anyhow!("cannot connect to anontokyo daemon at {path:?}: {e}"))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    let mut writer = stream.try_clone()?;
    writer.write_all(line.as_bytes())?;
    writer.flush()?;
    let mut reader = std::io::BufReader::new(stream);
    let mut out = String::new();
    reader.read_line(&mut out)?;
    let resp: Response = serde_json::from_str(out.trim())
        .map_err(|e| anyhow::anyhow!("bad daemon response {out:?}: {e}"))?;
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_shared(name: &str) -> Arc<Shared> {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Shared::load(dir.join("config.toml"))
    }

    #[test]
    fn update_mutates_persists_and_rebuilds_chain() {
        let shared = test_shared("ctl-update");
        let resp = handle(
            &shared,
            Request::SetBoost {
                which: BoostKind::Bass,
                enabled: Some(true),
                gain_db: Some(9.0),
                freq_hz: None,
                q: None,
            },
        )
        .0;
        assert!(resp.ok, "{resp:?}");
        let s = resp.settings.unwrap();
        assert!(s.bass.enabled);
        assert_eq!(s.bass.gain_db, 9.0);
        assert!(!shared.chain.load().stages.is_empty());
        // persisted
        let on_disk = settings::load_settings(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target/test-tmp")
                .join(format!("ctl-update-{}", std::process::id()))
                .join("config.toml"),
        );
        assert!(on_disk.bass.enabled);
    }

    #[test]
    fn out_of_range_band_is_rejected_then_clamped() {
        let shared = test_shared("ctl-band");
        let (resp, _) = handle(
            &shared,
            Request::SetBand {
                mode: EqMode::Multi,
                index: 999,
                freq_hz: None,
                gain_db: Some(3.0),
                q: None,
            },
        );
        assert!(!resp.ok);
        let (resp, _) = handle(
            &shared,
            Request::SetBand {
                mode: EqMode::Multi,
                index: 0,
                freq_hz: Some(1.0), // below FREQ_MIN
                gain_db: None,
                q: None,
            },
        );
        assert!(resp.ok);
        assert_eq!(
            resp.settings.unwrap().multi_bands[0].freq_hz,
            settings::FREQ_MIN
        );
    }

    #[test]
    fn stop_flag_flips() {
        let shared = test_shared("ctl-stop");
        let (resp, stop) = handle(&shared, Request::Stop);
        assert!(resp.ok);
        assert!(stop);
        let (_, stop) = handle(&shared, Request::Status);
        assert!(!stop);
    }

    #[test]
    fn json_roundtrip_of_every_request() {
        let reqs = vec![
            Request::Status,
            Request::SetBypass { enabled: true },
            Request::SetPreamp { db: -3.5 },
            Request::SetLimiter { enabled: false },
            Request::SetBoost {
                which: BoostKind::Vocal,
                enabled: Some(false),
                gain_db: Some(5.0),
                freq_hz: Some(2500.0),
                q: Some(1.2),
            },
            Request::SetEqMode {
                mode: EqMode::Multi,
            },
            Request::SetBand {
                mode: EqMode::Simple,
                index: 1,
                freq_hz: Some(900.0),
                gain_db: None,
                q: Some(2.0),
            },
            Request::SetBandCount { count: 31 },
            Request::SavePreset {
                name: "flat".into(),
            },
            Request::LoadPreset {
                name: "bass".into(),
            },
            Request::ListPresets,
            Request::Stop,
        ];
        for r in reqs {
            let json = serde_json::to_string(&r).unwrap();
            let back: Request = serde_json::from_str(&json).unwrap();
            let _ = format!("{back:?}");
        }
    }
}
