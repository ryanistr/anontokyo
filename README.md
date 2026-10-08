# AnonTokyo

Fully-Rust system-wide audio effects daemon for PipeWire: Bass / Vocal / Treble
boost toggles combined with a customizable EQ (simple 3-band or multi-band up
to 20 bands), presets, preamp, and a soft limiter. Functions first — the GUI
comes later; everything is driven by `anonctl`.

## Build & test

    cargo build --release
    cargo test            # 25 unit tests (DSP math, settings, control protocol)

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
    anonctl eq mode simple|multi
    anonctl eq set low|mid|high --gain -6 --freq 120        simple mode
    anonctl eq set m0..m19 --gain 6 --freq 500 --q 1.4      multi mode
    anonctl eq count 4..20              multi band count at runtime
    anonctl preset save|load|delete <name>; anonctl presets

Chain order: preamp -> bass -> vocal -> treble -> EQ (mode-dependent) -> soft
limiter (knee 0.85, ceiling 0.999 = -0.009 dBFS). DSP: RBJ Audio-EQ-Cookbook
biquads, f64 coefficients / f32 samples, lock-free settings swap via arc-swap.

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
| limiter, +20.7 dB over 0 dBFS in  | -0.0085 dB (ceiling)     | cap   | 0.0002 |

(A/B recordings also caught real app audio — Zen/YouTube — flowing through the
chain into the hardware sink, confirming system-wide routing.)

## Config

`~/.config/anontokyo/config.toml` (persisted on every change) and presets at
`~/.config/anontokyo/presets/<name>.toml`.
