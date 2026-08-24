//! The Tauri backend binary (spec 3.4 M8). Wires `loomix_app::control`'s
//! bridge to a running `Engine` and exposes it to the React frontend as a
//! set of `#[tauri::command]`s.
//!
//! **Deliberately not wired to real device I/O yet.** `spawn_audio_thread`
//! below simulates the audio thread with a timer-paced loop and a
//! synthetic 440Hz test tone on strip 0, not `loomix-hal`'s real IOProc
//! callbacks (`engine_io`/`device_wiring`, already built in M4). This
//! proves the whole UI <-> bridge <-> engine loop end to end -- live
//! meters, mute/solo/fader/bus-mode controls actually reaching a real
//! `Engine` -- without also taking on live CoreAudio device selection in
//! the same pass; that's the explicit next step, not a silently dropped
//! part of spec 3.4 M8's scope. See `docs/ARCHITECTURE.md`.

use loomix_app::control::{
    self, BusSnapshot, CommandSink, ControlSnapshot, EngineCommand, LatestValueReader,
    MeterSnapshot, StripSnapshot,
};
use loomix_core::bus::BusMono;
use loomix_core::bus_mode::BusMode;
use loomix_core::{Engine, Frame, CHANNELS, NUM_BUSES, NUM_STRIPS};
use std::sync::Mutex;
use std::time::Duration;
use tauri::State;

const RECONCILE_QUEUE_CAPACITY: usize = 8;
const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK_LEN: usize = 128;
const TEST_TONE_HZ: f32 = 440.0;

struct AppState {
    sink: Mutex<CommandSink>,
    control_reader: Mutex<LatestValueReader<ControlSnapshot>>,
    meter_reader: Mutex<LatestValueReader<MeterSnapshot>>,
}

fn bus_mono_to_str(mono: BusMono) -> &'static str {
    match mono {
        BusMono::Off => "off",
        BusMono::Mono => "mono",
        BusMono::StereoReverse => "stereo_reverse",
    }
}

fn bus_mono_from_str(s: &str) -> Option<BusMono> {
    match s {
        "off" => Some(BusMono::Off),
        "mono" => Some(BusMono::Mono),
        "stereo_reverse" => Some(BusMono::StereoReverse),
        _ => None,
    }
}

/// spec 1.6's 12 modes, snake_case for the wire format.
fn bus_mode_to_str(mode: BusMode) -> &'static str {
    match mode {
        BusMode::Normal => "normal",
        BusMode::MixDownA => "mix_down_a",
        BusMode::MixDownB => "mix_down_b",
        BusMode::StereoRepeat => "stereo_repeat",
        BusMode::Composite => "composite",
        BusMode::UpMixTv => "up_mix_tv",
        BusMode::UpMix21 => "up_mix_2_1",
        BusMode::UpMix41 => "up_mix_4_1",
        BusMode::UpMix61 => "up_mix_6_1",
        BusMode::CenterOnly => "center_only",
        BusMode::LfeOnly => "lfe_only",
        BusMode::RearOnly => "rear_only",
    }
}

fn bus_mode_from_str(s: &str) -> Option<BusMode> {
    Some(match s {
        "normal" => BusMode::Normal,
        "mix_down_a" => BusMode::MixDownA,
        "mix_down_b" => BusMode::MixDownB,
        "stereo_repeat" => BusMode::StereoRepeat,
        "composite" => BusMode::Composite,
        "up_mix_tv" => BusMode::UpMixTv,
        "up_mix_2_1" => BusMode::UpMix21,
        "up_mix_4_1" => BusMode::UpMix41,
        "up_mix_6_1" => BusMode::UpMix61,
        "center_only" => BusMode::CenterOnly,
        "lfe_only" => BusMode::LfeOnly,
        "rear_only" => BusMode::RearOnly,
        _ => return None,
    })
}

#[derive(serde::Serialize)]
struct StripSnapshotDto {
    mute: bool,
    solo: bool,
    mono: bool,
    bus_assign: [bool; NUM_BUSES],
    gain_layer_db: [f32; NUM_BUSES],
}

impl From<StripSnapshot> for StripSnapshotDto {
    fn from(s: StripSnapshot) -> Self {
        Self {
            mute: s.mute,
            solo: s.solo,
            mono: s.mono,
            bus_assign: s.bus_assign,
            gain_layer_db: s.gain_layer_db,
        }
    }
}

#[derive(serde::Serialize)]
struct BusSnapshotDto {
    mute: bool,
    mono: &'static str,
    mode: &'static str,
    gain_db: f32,
}

impl From<BusSnapshot> for BusSnapshotDto {
    fn from(b: BusSnapshot) -> Self {
        Self {
            mute: b.mute,
            mono: bus_mono_to_str(b.mono),
            mode: bus_mode_to_str(b.mode),
            gain_db: b.gain_db,
        }
    }
}

#[derive(serde::Serialize)]
struct ControlSnapshotDto {
    strips: Vec<StripSnapshotDto>,
    buses: Vec<BusSnapshotDto>,
}

impl From<ControlSnapshot> for ControlSnapshotDto {
    fn from(s: ControlSnapshot) -> Self {
        Self {
            strips: s.strips.into_iter().map(Into::into).collect(),
            buses: s.buses.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(serde::Serialize)]
struct MeterSnapshotDto {
    strips: Vec<[f32; CHANNELS]>,
    buses: Vec<[f32; CHANNELS]>,
}

impl From<MeterSnapshot> for MeterSnapshotDto {
    fn from(m: MeterSnapshot) -> Self {
        let channels = |meter: &loomix_core::Meter| -> [f32; CHANNELS] {
            std::array::from_fn(|c| meter.peak(c))
        };
        Self {
            strips: m.strips.iter().map(channels).collect(),
            buses: m.buses.iter().map(channels).collect(),
        }
    }
}

#[tauri::command]
fn get_control_snapshot(state: State<AppState>) -> ControlSnapshotDto {
    state.control_reader.lock().unwrap().read().into()
}

#[tauri::command]
fn get_meters(state: State<AppState>) -> MeterSnapshotDto {
    state.meter_reader.lock().unwrap().read().into()
}

/// Enqueues and immediately flushes: a plain button/dropdown/slider
/// commit is already a discrete, infrequent event, so there's no
/// coalescing benefit to batching across a timer tick the way a
/// continuous fader-drag UI eventually will (`docs/ARCHITECTURE.md`).
fn send(state: &State<AppState>, command: EngineCommand) {
    let mut sink = state.sink.lock().unwrap();
    sink.enqueue(command);
    sink.flush();
}

#[tauri::command]
fn set_strip_mute(state: State<AppState>, strip: usize, on: bool) {
    send(&state, EngineCommand::SetStripMute(strip, on));
}

#[tauri::command]
fn set_strip_solo(state: State<AppState>, strip: usize, on: bool) {
    send(&state, EngineCommand::SetStripSolo(strip, on));
}

#[tauri::command]
fn set_strip_mono(state: State<AppState>, strip: usize, on: bool) {
    send(&state, EngineCommand::SetStripMono(strip, on));
}

#[tauri::command]
fn set_strip_bus_assign(state: State<AppState>, strip: usize, bus: usize, on: bool) {
    send(&state, EngineCommand::SetStripBusAssign(strip, bus, on));
}

#[tauri::command]
fn set_strip_gain_layer(state: State<AppState>, strip: usize, bus: usize, db: f32) {
    send(&state, EngineCommand::SetStripGainLayer(strip, bus, db));
}

#[tauri::command]
fn set_bus_mute(state: State<AppState>, bus: usize, on: bool) {
    send(&state, EngineCommand::SetBusMute(bus, on));
}

#[tauri::command]
fn set_bus_mono(state: State<AppState>, bus: usize, mono: String) -> Result<(), String> {
    let mono = bus_mono_from_str(&mono).ok_or_else(|| format!("unknown mono mode: {mono}"))?;
    send(&state, EngineCommand::SetBusMono(bus, mono));
    Ok(())
}

#[tauri::command]
fn set_bus_mode(state: State<AppState>, bus: usize, mode: String) -> Result<(), String> {
    let mode = bus_mode_from_str(&mode).ok_or_else(|| format!("unknown bus mode: {mode}"))?;
    send(&state, EngineCommand::SetBusMode(bus, mode));
    Ok(())
}

#[tauri::command]
fn set_bus_gain(state: State<AppState>, bus: usize, db: f32) {
    send(&state, EngineCommand::SetBusGain(bus, db));
}

/// Simulates the real-time thread: never blocks on anything but its own
/// pacing sleep, drains commands and calls `process_block` exactly the
/// way a real IOProc callback would (`docs/ARCHITECTURE.md`'s M8 entries
/// on why this isn't real device I/O yet). Owns `Engine` exclusively --
/// nothing outside this thread ever touches it, only the two bridge
/// channels.
fn spawn_audio_thread(
    mut drain: control::CommandDrain,
    mut control_pub: control::LatestValuePublisher<ControlSnapshot>,
    mut meter_pub: control::LatestValuePublisher<MeterSnapshot>,
) {
    std::thread::spawn(move || {
        let mut engine = Engine::new();
        engine.set_sample_rate(SAMPLE_RATE);
        let block_duration = Duration::from_secs_f32(BLOCK_LEN as f32 / SAMPLE_RATE);
        let mut phase = 0.0f32;

        loop {
            drain.drain_into(&mut engine, 64);

            let mut inputs: Vec<Vec<Frame>> = (0..NUM_STRIPS)
                .map(|_| vec![[0.0; CHANNELS]; BLOCK_LEN])
                .collect();
            for frame in inputs[0].iter_mut() {
                let sample = (phase * std::f32::consts::TAU).sin() * 0.4;
                frame[0] = sample;
                frame[1] = sample;
                phase = (phase + TEST_TONE_HZ / SAMPLE_RATE).fract();
            }
            let input_refs: Vec<&[Frame]> = inputs.iter().map(|v| v.as_slice()).collect();
            let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; BLOCK_LEN]; NUM_BUSES];
            {
                let mut out_refs: Vec<&mut [Frame]> =
                    out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();
                engine.process_block(&input_refs, &mut out_refs);
            }

            control_pub.publish(ControlSnapshot::capture(&engine));
            meter_pub.publish(MeterSnapshot::capture(&engine));

            std::thread::sleep(block_duration);
        }
    });
}

fn main() {
    let (sink, drain) = control::control_channel();
    let (control_publisher, control_reader) = control::snapshot_channel(RECONCILE_QUEUE_CAPACITY);
    let (meter_publisher, meter_reader) =
        control::latest_value_channel::<MeterSnapshot>(RECONCILE_QUEUE_CAPACITY);

    spawn_audio_thread(drain, control_publisher, meter_publisher);

    tauri::Builder::default()
        .manage(AppState {
            sink: Mutex::new(sink),
            control_reader: Mutex::new(control_reader),
            meter_reader: Mutex::new(meter_reader),
        })
        .invoke_handler(tauri::generate_handler![
            get_control_snapshot,
            get_meters,
            set_strip_mute,
            set_strip_solo,
            set_strip_mono,
            set_strip_bus_assign,
            set_strip_gain_layer,
            set_bus_mute,
            set_bus_mono,
            set_bus_mode,
            set_bus_gain,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Loomix app");
}
