# AnonTokyo

Fully-Rust system-wide audio effects daemon for PipeWire: Bass / Vocal / Treble
boost toggles combined with a customizable EQ (simple 3-band or multi-band up
to 32 bands), five FxSound-style boost sliders (clarity, ambience, surround,
dynamic boost, bass boost), presets including the 12 official FxSound factory
presets, preamp, and a soft limiter. Functions first — the GUI comes later;
everything is driven by `anonctl` and described by a machine-readable schema.

## Build & test

    cargo build --release
    cargo test            # 45 unit tests (DSP math, settings, presets, protocol)

## Run

    ./target/release/anontokyo    # daemon: routes ALL system audio through the chain
    ./target/release/anonctl ...  # control client (unix socket, live-safe)

    anonctl stop                  # restore default sink, remove virtual sink, exit

## Data flow

    apps -> anontokyo.in (null sink) -> capture (monitor) -> DSP -> playback -> hw sink

Wiring requirements learned the hard way (WirePlumber 0.5.18 policy):

- capture stream: `target.object = <sink node name>` (a ".monitor" name matches
  no node — monitors are ports, not nodes) **and** `stream.capture.sink=true`
- playback stream: `target.object = <hw sink name>`
- both streams: `node.dont-reconnect=true` — without it WP marks "follow
  default" at link time and moves the stream when the default sink switches

## Control surface

    anonctl status | --json status      live state (stages active, format, settings)
    anonctl bypass on|off               hard bypass (bit-exact passthrough)
    anonctl preamp --db -3              -12..+12 dB (negative values accepted)
    anonctl boost bass|vocal|treble --enable|--disable --gain -6 --freq 150 --q 0.7
    anonctl fx clarity|surround|ambience|dynamic-boost|bass-boost
                                        --enable|--disable --amount 0..100
    anonctl eq mode simple|multi
    anonctl eq set low|mid|high --gain -6 --freq 120        simple mode
    anonctl eq set m0..m31 --gain 6 --freq 500 --q 1.4      multi mode
    anonctl eq count 4..32              multi band count at runtime
    anonctl preset save|load|delete|rename <name>; anonctl presets
    anonctl preset import <file.fac>    convert a FxSound preset to a user preset
    anonctl describe                    JSON control schema for the future GUI

Chain order: preamp -> clarity -> bass-boost slider -> bass -> vocal -> treble
-> EQ (mode-dependent) -> dynamic boost -> surround -> ambience -> soft limiter
(knee 0.85, ceiling 0.999 = -0.009 dBFS). DSP: RBJ Audio-EQ-Cookbook biquads,
f64 coefficients / f32 samples, lock-free settings swap via arc-swap.

## FxSound-style effects

Five continuous 0..100 sliders that stack with (never replace) the three boost
toggles; the parameter model mirrors FxSound's (5 effect slots + on/off +
10-band graphic EQ in presets):

| slider        | DSP                                        | amount 100  |
|---------------|--------------------------------------------|-------------|
| clarity       | peaking @ 4 kHz (FxSound "Fidelity")       | +9 dB       |
| bass-boost    | low shelf @ 100 Hz (separate from `boost bass`) | +12 dB  |
| dynamic-boost | upward compressor, target -26 dB, 5/150 ms envelope | up to +12 dB |
| surround      | M/S stereo width                           | 2x side     |
| ambience      | 4-tap early reflections (17/29/41/53 ms, right channel stretched) | wet 0.35 |

Presets follow FxSound's split: the 12 factory presets ship as their original
`.fac` files (`presets-fx/`, parsed at load time) and are always available;
user presets in `~/.config/anontokyo/presets/` shadow factory presets of the
same name and are the only ones that can be deleted or renamed. `preset
import` converts any other FxSound `.fac` file. Factory preset values derive
from the FxSound project (https://github.com/fxsoundapp/fxsound).

`anonctl --json describe` returns groups/keys/types/ranges for every control,
so a future GUI builds its widgets from data instead of hardcoded constants.

## Verified end-to-end (48 kHz live graph, hw monitor recordings)

| test                              | measured | predicted | error  |
|-----------------------------------|----------|-----------|--------|
| flat passthrough                  | exact match to source (both ch) | 0.000 |        |
| bass +10 @150 -> 100 Hz           | +8.16 dB | +8.07     | 0.09   |
| vocal +8 @3k (band center)        | +8.00 dB | +8.00     | 0.0005 |
| treble +8 @9k -> 10k              | +5.10 dB | +5.19     | 0.09   |
| multi m0 +6 @500 (center)         | +6.00 dB | +6.00     | 0.0004 |
| simple low -6 @150 -> 100 Hz      | -4.97 dB | -4.97     | 0.005  |
| bass+low stacked                  | +8.31 dB | +8.29     | 0.017  |
| preamp -3                         | -3.00 dB | -3.00     | 0.001  |
| bypass on                         | exact baseline           | 0.000 |        |
| bass-boost slider 100 -> 100 Hz corner | +6.09 dB            | +6.00 | 0.09   |
| flat after fx-era rework (both runs) | exact match to source   | 0.000 |        |
| limiter, +20.7 dB over 0 dBFS in  | -0.0085 dB (ceiling)     | cap   | 0.0002 |

(A/B recordings also caught real app audio — Zen/YouTube — flowing through the
chain into the hardware sink, confirming system-wide routing.)

## Config

`~/.config/anontokyo/config.toml` (persisted on every change) and user presets
at `~/.config/anontokyo/presets/<name>.toml`; factory presets live in
`presets-fx/` inside the repo and are embedded in the binary. Configs written
before the fx fields existed still load (missing fields default to off/0).
