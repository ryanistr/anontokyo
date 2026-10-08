//! anontokyo — the AnonTokyo daemon.
//!
//! Graph: apps -> [anontokyo.in null sink] -> capture stream (monitor) ->
//! DSP -> ring buffer -> playback stream -> hardware sink.

use anontokyo::ctl::{self, AudioFormat};
use anontokyo::{ChainStates, Shared, admin, settings};
use anyhow::{Context, Result};
use clap::Parser;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::audio::{AudioFormat as SpaAudioFormat, AudioInfoRaw, MAX_CHANNELS};
use spa::pod::Pod;
use std::cell::RefCell;
use std::convert::TryInto;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// f32 slots in the capture->playback ring (~0.68 s of stereo 48k)
const RING_SAMPLES: usize = 65_536;
/// latency ceiling: playback trims the ring back down to this when above it
const SOFT_CAP_SAMPLES: usize = 16_384;
const STARTUP_SETTLE: Duration = Duration::from_millis(1500);
const WATCHDOG_TICK: Duration = Duration::from_millis(200);

#[derive(Parser)]
#[command(
    name = "anontokyo",
    about = "AnonTokyo daemon: system-wide bass/vocal/treble boosts + parametric EQ for PipeWire"
)]
struct Args {
    /// name of the virtual sink apps play into
    #[arg(long, default_value = "anontokyo.in")]
    sink: String,

    /// hardware sink to play processed audio into (default: current default sink)
    #[arg(long)]
    hw_sink: Option<String>,

    /// do not switch the system default sink to the virtual sink
    #[arg(long)]
    no_default_switch: bool,
}

#[derive(Default)]
struct StreamHealth {
    capture: String,
    playback: String,
}

impl StreamHealth {
    fn all_connected(&self) -> bool {
        let ok = |s: &str| matches!(s, "Paused" | "Streaming" | "Connecting");
        !self.capture.is_empty()
            && !self.playback.is_empty()
            && ok(&self.capture)
            && ok(&self.playback)
            && self.capture != "Error"
            && self.playback != "Error"
    }
}

fn format_pod_values() -> Result<Vec<u8>> {
    let mut info = AudioInfoRaw::new();
    info.set_format(SpaAudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(CHANNELS as u32);
    let mut position = [0; MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }),
    )
    .map_err(|e| anyhow::anyhow!("pod serialize: {e:?}"))?
    .0
    .into_inner();
    Ok(values)
}

fn run(args: Args, shared: Arc<Shared>, graph: Rc<RefCell<admin::GraphSetup>>) -> Result<()> {
    let hw_sink = admin::resolve_hw_sink(args.hw_sink.as_deref(), &args.sink)?;
    log::info!("processing into hardware sink: {hw_sink}");

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).context("pipewire main loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None).context("pipewire context")?;
    let core = context.connect_rc(None).context("connect to pipewire")?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    {
        let flag = stop_flag.clone();
        ctrlc::set_handler(move || {
            log::info!("signal received, shutting down");
            flag.store(true, Ordering::SeqCst);
        })
        .context("install signal handler")?;
    }

    let health = Arc::new(Mutex::new(StreamHealth::default()));
    let (producer, consumer) = rtrb::RingBuffer::<f32>::new(RING_SAMPLES);
    let occupancy = Arc::new(AtomicU64::new(0));

    // WP matches target.object against node.name: a ".monitor" name matches no node.
    // stream.capture.sink=true is what makes a sink acceptable as a capture target.
    let capture_props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::TARGET_OBJECT => args.sink.as_str(),
        *pw::keys::STREAM_CAPTURE_SINK => "true",
        // without this, linking to the then-default marks the stream "follow
        // default" and the later default switch drags it to our own sink
        *pw::keys::NODE_DONT_RECONNECT => "true",
    };
    let playback_props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::TARGET_OBJECT => hw_sink.as_str(),
        *pw::keys::NODE_DONT_RECONNECT => "true",
    };

    struct CapData {
        shared: Arc<Shared>,
        channels: AtomicU32,
        states: ChainStates,
        scratch: Vec<f32>,
        producer: rtrb::Producer<f32>,
        occupancy: Arc<AtomicU64>,
        health: Arc<Mutex<StreamHealth>>,
        dropped: u64,
    }

    struct PlayData {
        consumer: rtrb::Consumer<f32>,
        occupancy: Arc<AtomicU64>,
        scratch: Vec<f32>,
        trim: Vec<f32>,
        health: Arc<Mutex<StreamHealth>>,
    }

    let capture = pw::stream::StreamBox::new(&core, "anontokyo-capture", capture_props)?;
    let playback = pw::stream::StreamBox::new(&core, "anontokyo-playback", playback_props)?;

    let cap_data = CapData {
        shared: shared.clone(),
        channels: AtomicU32::new(CHANNELS as u32),
        states: ChainStates::default(),
        scratch: Vec::new(),
        producer,
        occupancy: occupancy.clone(),
        health: health.clone(),
        dropped: 0,
    };
    let cap_listener = capture
        .add_local_listener_with_user_data(cap_data)
        .state_changed(|_, data, old, new| {
            log::info!("capture stream: {old:?} -> {new:?}");
            let label = match &new {
                pw::stream::StreamState::Error(e) => format!("Error: {e}"),
                other => format!("{other:?}"),
            };
            let label = label.split(':').next().unwrap_or("Unknown").to_string();
            if let Ok(mut h) = data.health.lock() {
                h.capture = label;
            }
        })
        .param_changed(|_, data, id, param| {
            if param.is_none() || id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = AudioInfoRaw::new();
            if info.parse(param.expect("checked")).is_err() {
                return;
            }
            let fmt = AudioFormat {
                rate: info.rate(),
                channels: info.channels() as usize,
            };
            log::info!("capture format: {} Hz {} ch", fmt.rate, fmt.channels);
            data.channels.store(fmt.channels as u32, Ordering::Relaxed);
            data.shared.set_format(fmt);
        })
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let d = &mut datas[0];
            let (offset, size) = {
                let c = d.chunk();
                (c.offset() as usize, c.size() as usize)
            };
            if size == 0 {
                return;
            }
            let Some(slice) = d.data() else {
                return;
            };
            let end = offset.saturating_add(size);
            if end > slice.len() {
                return;
            }
            let src = &slice[offset..end];
            let ch = data.channels.load(Ordering::Relaxed).max(1) as usize;
            let frame_bytes = 4 * ch;
            let frames = src.len() / frame_bytes;
            if frames == 0 {
                return;
            }
            let needed = frames * ch;
            if data.scratch.len() < needed {
                data.scratch.resize(needed, 0.0);
            }
            {
                let buf = &mut data.scratch[..needed];
                for (s, b) in buf.iter_mut().zip(src.chunks_exact(4)) {
                    let arr: [u8; 4] = b.try_into().expect("chunks_exact(4)");
                    *s = f32::from_le_bytes(arr);
                }
                let chain = data.shared.chain.load();
                chain.process(&mut data.states, buf);
            }
            let buf = &data.scratch[..needed];
            let occ = data.occupancy.load(Ordering::Relaxed);
            if occ + needed as u64 <= RING_SAMPLES as u64 {
                let mut pushed = 0usize;
                for &s in buf {
                    if data.producer.push(s).is_ok() {
                        pushed += 1;
                    } else {
                        break;
                    }
                }
                data.occupancy.fetch_add(pushed as u64, Ordering::Relaxed);
                if pushed != needed {
                    data.dropped += 1;
                }
            } else {
                data.dropped += 1;
            }
            if data.dropped > 0 && data.dropped % 100 == 1 {
                log::warn!("ring full, dropped {} chunks so far", data.dropped);
            }
        })
        .register()
        .context("register capture listener")?;

    let play_data = PlayData {
        consumer,
        occupancy: occupancy.clone(),
        scratch: Vec::new(),
        trim: Vec::new(),
        health: health.clone(),
    };
    let play_listener = playback
        .add_local_listener_with_user_data(play_data)
        .state_changed(|_, data, old, new| {
            log::info!("playback stream: {old:?} -> {new:?}");
            let label = match &new {
                pw::stream::StreamState::Error(e) => format!("Error: {e}"),
                other => format!("{other:?}"),
            };
            let label = label.split(':').next().unwrap_or("Unknown").to_string();
            if let Ok(mut h) = data.health.lock() {
                h.playback = label;
            }
        })
        .param_changed(|_, _data, _id, _param| {})
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let d = &mut datas[0];
            let Some(slice) = d.data() else {
                return;
            };
            let max_frames = slice.len() / (4 * CHANNELS);
            if max_frames == 0 {
                return;
            }

            // latency trim: never let the ring hold more than the soft cap
            let occ = data.occupancy.load(Ordering::Relaxed);
            let excess = occ.saturating_sub(SOFT_CAP_SAMPLES as u64) as usize;
            if excess > 0 {
                data.trim.resize(excess, 0.0);
                match data.consumer.pop() {
                    Ok(_) => {
                        data.occupancy.fetch_sub(1, Ordering::Relaxed);
                    }
                    Err(_) => break_trim(),
                }
                for _ in 1..excess {
                    if data.consumer.pop().is_err() {
                        break;
                    }
                    data.occupancy.fetch_sub(1, Ordering::Relaxed);
                }
            }

            data.scratch.resize(max_frames * CHANNELS, 0.0);
            let mut popped = 0usize;
            for s in data.scratch.iter_mut() {
                match data.consumer.pop() {
                    Ok(v) => {
                        *s = v;
                        popped += 1;
                    }
                    Err(_) => break,
                }
            }
            data.occupancy.fetch_sub(popped as u64, Ordering::Relaxed);

            let out_end = popped * 4;
            for (i, s) in data.scratch[..popped].iter().enumerate() {
                let b = s.to_le_bytes();
                slice[i * 4..i * 4 + 4].copy_from_slice(&b);
            }
            let _ = out_end;
            let chunk = d.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = (4 * CHANNELS) as _;
            *chunk.size_mut() = (popped * 4 * CHANNELS) as _;
        })
        .register()
        .context("register playback listener")?;

    let cap_values = format_pod_values()?;
    let mut cap_params = [Pod::from_bytes(&cap_values).expect("valid pod")];
    capture
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut cap_params,
        )
        .context("connect capture stream")?;

    let play_values = format_pod_values()?;
    let mut play_params = [Pod::from_bytes(&play_values).expect("valid pod")];
    playback
        .connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut play_params,
        )
        .context("connect playback stream")?;

    // one-shot: after the graph settles, decide whether to take over default sink
    let startup_timer = mainloop.loop_().add_timer({
        let health = health.clone();
        let graph = graph.clone();
        let shared = shared.clone();
        let stop_flag = stop_flag.clone();
        let no_switch = args.no_default_switch;
        move |_| {
            let healthy = health.lock().map(|h| h.all_connected()).unwrap_or(false);
            let h = health.lock().map(|h| (h.capture.clone(), h.playback.clone())).unwrap_or_default();
            log::info!("stream health after settle: capture={} playback={}", h.0, h.1);
            if !healthy {
                log::error!("streams not healthy - default sink NOT switched (apps keep playing to hardware)");
                return;
            }
            if no_switch {
                log::info!("--no-default-switch set, not taking over default sink");
                return;
            }
            match graph.borrow_mut().switch_default() {
                Ok(()) => {
                    let s = shared.snapshot();
                    log::info!(
                        "ready: bypass={} active_stages={}",
                        s.bypass,
                        shared.chain.load().stages.len()
                    );
                }
                Err(e) => log::error!("failed to switch default sink: {e}"),
            }
            let _ = &stop_flag;
        }
    });
    startup_timer
        .update_timer(Some(STARTUP_SETTLE), None)
        .into_result()
        .context("arm startup timer")?;

    // watchdog: quit the main loop when the stop flag is raised
    let watchdog = mainloop.loop_().add_timer({
        let flag = stop_flag.clone();
        let ml = mainloop.clone();
        move |_| {
            if flag.load(Ordering::SeqCst) {
                ml.quit();
            }
        }
    });
    watchdog
        .update_timer(Some(WATCHDOG_TICK), Some(WATCHDOG_TICK))
        .into_result()
        .context("arm watchdog timer")?;

    let flag_for_ctl = stop_flag.clone();
    let flag_for_stop = stop_flag.clone();
    let ctl_handle = ctl::serve(
        shared.clone(),
        move || flag_for_ctl.load(Ordering::SeqCst),
        move || {
            flag_for_stop.store(true, Ordering::SeqCst);
        },
    )
    .context("start control server")?;

    log::info!(
        "anontokyo running: sink={} hw={} (ctrl-c or `anonctl stop` to quit)",
        args.sink,
        hw_sink
    );
    mainloop.run();

    // ---- teardown: give audio back before anything else ----
    let _ = cap_listener.unregister();
    let _ = play_listener.unregister();
    capture.disconnect().ok();
    playback.disconnect().ok();
    graph.borrow().restore();
    stop_flag.store(true, Ordering::SeqCst);
    let _ = ctl_handle.join();
    Ok(())
}

/// placeholder used to keep the trim loop readable
fn break_trim() {}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let shared = Shared::load(settings::config_path());
    let graph = admin::GraphSetup::install(&args.sink)?;
    let graph = Rc::new(RefCell::new(graph));

    let result = run(args, shared, graph.clone());
    if let Err(e) = &result {
        log::error!("anontokyo failed: {e:#}");
        graph.borrow().restore();
    }
    result
}
