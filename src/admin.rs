//! Graph administration: virtual sink creation and default-sink handling.
//!
//! PipeWire does not expose a client-creatable null-sink factory
//! (`support.null-audio-sink` is only registered inside protocol-pulse), so
//! the two graph-admin operations below go through `pactl`, the proven path
//! on this system. Everything else — DSP, daemon, control protocol — is pure
//! Rust over the native PipeWire bindings.

use anyhow::{Context, Result, bail};

fn pactl(args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("pactl")
        .args(args)
        .output()
        .context("failed to run pactl — is pipewire-pulse running?")?;
    if !out.status.success() {
        bail!(
            "pactl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn default_sink() -> Option<String> {
    let info = pactl(&["info"]).ok()?;
    info.lines().find_map(|l| {
        l.strip_prefix("Default Sink: ")
            .map(|s| s.trim().to_string())
    })
}

pub fn list_sinks() -> Vec<String> {
    pactl(&["list", "short", "sinks"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1).map(String::from))
        .collect()
}

pub fn sink_exists(name: &str) -> bool {
    list_sinks().iter().any(|s| s == name)
}

fn create_null_sink(name: &str) -> Result<u32> {
    let desc = "AnonTokyo Processed Audio";
    let id = pactl(&[
        "load-module",
        "module-null-sink",
        &format!("sink_name={name}"),
        &format!("sink_properties=device.description={desc}"),
    ])
    .context("load-module module-null-sink")?;
    let id = id
        .trim()
        .parse::<u32>()
        .context("bad module id from pactl")?;
    log::info!("created virtual sink {name} (module {id})");
    Ok(id)
}

/// What our daemon did to the graph, so `restore()` can undo exactly that.
pub struct GraphSetup {
    pub our_sink: String,
    pub module_id: Option<u32>,
    pub prev_default: Option<String>,
    pub switched_default: bool,
}

impl GraphSetup {
    /// Ensure the virtual sink exists (and remember how to remove it).
    /// Does NOT touch the default sink — that happens after streams are up.
    pub fn install(our_sink: &str) -> Result<Self> {
        let existed = sink_exists(our_sink);
        let module_id = if existed {
            log::info!("reusing existing sink {our_sink}");
            None
        } else {
            Some(create_null_sink(our_sink)?)
        };
        Ok(Self {
            our_sink: our_sink.to_string(),
            module_id,
            prev_default: default_sink(),
            switched_default: false,
        })
    }

    pub fn switch_default(&mut self) -> Result<()> {
        pactl(&["set-default-sink", &self.our_sink]).context("set-default-sink")?;
        self.switched_default = true;
        log::info!("default sink -> {}", self.our_sink);
        Ok(())
    }

    /// Best-effort teardown: give audio back to the user's real sink first.
    pub fn restore(&self) {
        if self.switched_default {
            if let Some(prev) = &self.prev_default {
                if prev != &self.our_sink && sink_exists(prev) {
                    if let Err(e) = pactl(&["set-default-sink", prev]) {
                        log::warn!("could not restore default sink: {e}");
                    } else {
                        log::info!("default sink restored -> {prev}");
                    }
                }
            }
        }
        if let Some(id) = self.module_id {
            if let Err(e) = pactl(&["unload-module", &id.to_string()]) {
                log::warn!("could not unload virtual sink module {id}: {e}");
            } else {
                log::info!("virtual sink removed (module {id})");
            }
        }
    }
}

/// Resolve which hardware sink processed audio should be played into.
/// Must be called BEFORE switching the default sink away from it.
pub fn resolve_hw_sink(arg: Option<&str>, our_sink: &str) -> Result<String> {
    if let Some(name) = arg {
        if !sink_exists(name) {
            bail!("hardware sink {name:?} does not exist");
        }
        return Ok(name.to_string());
    }
    if let Some(def) = default_sink() {
        if def != our_sink {
            return Ok(def);
        }
    }
    list_sinks()
        .into_iter()
        .find(|s| s != our_sink)
        .context("no hardware sink available to play into")
}
