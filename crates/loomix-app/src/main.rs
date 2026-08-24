//! The Tauri backend binary (spec 3.4 M8). Wires `loomix_app::control`'s
//! bridge to a running `Engine` and exposes it to the React frontend as a
//! set of `#[tauri::command]`s.
//!
//! **Real CoreAudio device I/O**, replacing the synthetic test tone the
//! first version of this file used to prove the UI <-> bridge <-> engine
//! loop end to end without also taking on live device (re)registration in
//! the same pass. That pass is over: [`connect_audio`] wires a real
//! output device as the clock master (spec 1.19 -- its bus is always A1)
//! and, optionally, a real input device into strip 0, using exactly
//! `loomix-app::device_wiring`'s functions and `loomix-soak`'s proven
//! ordering (every non-master device attached first, the master attached
//! last since it takes the driver by value and starts running
//! immediately) -- no new device-I/O logic, only wiring already-tested
//! pieces together for the first time from a live UI action instead of a
//! manual soak run. See `docs/ARCHITECTURE.md` for what was verified on
//! the host before this ever touched a real device, and why.
//!
//! There is no engine running at all until [`connect_audio`] succeeds --
//! [`AppState::session`] is `None` until then, and every command below
//! degrades to an inert default rather than panicking on a device that
//! was never selected.

use loomix_app::control::{
    self, BusSnapshot, CommandSink, ControlSnapshot, EngineCommand, LatestValueReader,
    MeterSnapshot, StripSnapshot,
};
use loomix_app::device_wiring::attach_capture_device;
use loomix_app::engine_io::{select_clock_master, DropoutCounter, EngineIoDriver};
use loomix_core::bus::BusMono;
use loomix_core::bus_mode::BusMode;
use loomix_core::{Engine, CHANNELS, NUM_BUSES};
use loomix_hal::clock::{ClockSource, DeviceId};
use loomix_hal::device::{
    channel_count, device_name, device_uid, list_device_ids, nominal_sample_rate, Direction,
    MasterTickCallback,
};
use loomix_hal::device_lifecycle::{CaptureIoProcHandle, MasterIoProcHandle};
use loomix_hal::drift::{DriftCorrector, PiController};
use loomix_hal::master_clock::MasterClock;
use std::sync::{Arc, Mutex};
use tauri::State;

const RECONCILE_QUEUE_CAPACITY: usize = 8;
/// Matches `loomix-soak`'s already-proven values exactly, not retuned
/// here: `RING_CAPACITY` comfortably absorbs scheduling jitter,
/// `MAX_BLOCK_FRAMES` covers spec 1.11's full buffer-size range
/// (128..2048 samples) so `EngineIoDriver`'s pre-allocated scratch
/// buffers never need to grow inside a real callback.
const RING_CAPACITY: usize = 1 << 16;
const MAX_BLOCK_FRAMES: usize = 2048;
/// The strip a connected input device's captured audio lands on. Strip 0
/// (spec 1.1's "HW 1") was already the UI's implicit "the active strip"
/// convention from the synthetic-tone version this replaces.
const INPUT_STRIP: usize = 0;

/// Everything a live audio connection owns: the two bridge halves the
/// Tauri commands below talk to, and the device handles that keep the
/// real I/O running -- dropping either handle stops and unregisters that
/// device (`loomix-hal`'s own `Drop` impls), which is exactly what
/// [`disconnect_audio`] relies on rather than any explicit teardown call.
struct AudioSession {
    sink: CommandSink,
    control_reader: LatestValueReader<ControlSnapshot>,
    meter_reader: LatestValueReader<MeterSnapshot>,
    capture_underruns: Option<DropoutCounter>,
    _capture: Option<CaptureIoProcHandle>,
    _master: MasterIoProcHandle,
}

struct AppState {
    session: Mutex<Option<AudioSession>>,
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

#[derive(serde::Serialize)]
struct DeviceInfoDto {
    uid: String,
    name: String,
    input_channels: usize,
    output_channels: usize,
}

#[derive(serde::Serialize)]
struct AudioStatusDto {
    connected: bool,
    /// `None` when connected with no input device attached, not just
    /// "zero so far" -- the UI needs to tell "no input selected" apart
    /// from "input selected, draining cleanly".
    capture_underruns: Option<u64>,
}

#[tauri::command]
fn list_audio_devices() -> Result<Vec<DeviceInfoDto>, String> {
    let ids = list_device_ids().map_err(|e| format!("CoreAudio error {e}"))?;
    let mut out = Vec::new();
    for id in ids {
        let uid = device_uid(id).unwrap_or_default();
        if uid.is_empty() {
            continue; // an object CoreAudio listed but won't identify -- nothing to select
        }
        let input_channels = channel_count(id, Direction::Input).unwrap_or(0);
        let output_channels = channel_count(id, Direction::Output).unwrap_or(0);
        if input_channels == 0 && output_channels == 0 {
            continue;
        }
        out.push(DeviceInfoDto {
            uid,
            name: device_name(id).unwrap_or_default(),
            input_channels,
            output_channels,
        });
    }
    Ok(out)
}

#[tauri::command]
fn get_audio_status(state: State<AppState>) -> AudioStatusDto {
    let session = state.session.lock().unwrap();
    match session.as_ref() {
        Some(s) => AudioStatusDto {
            connected: true,
            capture_underruns: s.capture_underruns.as_ref().map(DropoutCounter::get),
        },
        None => AudioStatusDto {
            connected: false,
            capture_underruns: None,
        },
    }
}

#[tauri::command]
fn get_control_snapshot(state: State<AppState>) -> ControlSnapshotDto {
    let mut session = state.session.lock().unwrap();
    match session.as_mut() {
        Some(s) => s.control_reader.read().into(),
        None => ControlSnapshot::default().into(),
    }
}

#[tauri::command]
fn get_meters(state: State<AppState>) -> MeterSnapshotDto {
    let mut session = state.session.lock().unwrap();
    match session.as_mut() {
        Some(s) => s.meter_reader.read().into(),
        None => MeterSnapshot::default().into(),
    }
}

/// Enqueues and immediately flushes: a plain button/dropdown/slider
/// commit is already a discrete, infrequent event, so there's no
/// coalescing benefit to batching across a timer tick the way a
/// continuous fader-drag UI eventually will (`docs/ARCHITECTURE.md`).
/// A silent no-op when nothing is connected -- there's no engine for the
/// command to reach yet, and refusing every control loudly before the
/// user has picked a device would be noise, not useful feedback.
fn send(state: &State<AppState>, command: EngineCommand) {
    let mut session = state.session.lock().unwrap();
    if let Some(s) = session.as_mut() {
        s.sink.enqueue(command);
        s.sink.flush();
    }
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

fn resolve_uid(uid: &str) -> Result<DeviceId, String> {
    let ids = list_device_ids().map_err(|e| format!("CoreAudio error {e}"))?;
    for id in ids {
        if device_uid(id).map(|u| u == uid).unwrap_or(false) {
            return Ok(id);
        }
    }
    Err(format!("no device with UID '{uid}' found (it may have been unplugged since the picker was last refreshed)"))
}

/// Tears down any existing connection and wires a fresh one: `output_uid`
/// becomes the clock master (spec 1.19 -- its bus is always A1/bus 0),
/// and, if given, `input_uid` is attached as a drift-corrected capture
/// device feeding [`INPUT_STRIP`]. Mirrors `loomix-soak`'s exact,
/// already-proven ordering: every non-master device is attached first,
/// the master last, since `attach_master_device`'s underlying
/// `MasterIoProcHandle::start` takes the driver by value and starts it
/// running immediately (`docs/ARCHITECTURE.md`).
#[tauri::command]
fn connect_audio(
    state: State<AppState>,
    input_uid: Option<String>,
    output_uid: String,
) -> Result<(), String> {
    // Drop the old session (if any) before building the new one -- its
    // `Drop` impls stop and unregister the previous devices cleanly.
    *state.session.lock().unwrap() = None;

    let output_id = resolve_uid(&output_uid)?;
    let output_channels =
        channel_count(output_id, Direction::Output).map_err(|e| format!("CoreAudio error {e}"))?;
    if output_channels == 0 {
        return Err(format!("{output_uid} has no output channels"));
    }
    match select_clock_master(Some(output_id)).map_err(|e| format!("CoreAudio error {e}"))? {
        ClockSource::Device(id) if id == output_id => {}
        _ => return Err(format!("{output_uid} is not currently connected")),
    }
    let sample_rate = nominal_sample_rate(output_id).map_err(|e| format!("CoreAudio error {e}"))?;

    let mut engine = Engine::new();
    engine.set_sample_rate(sample_rate as f32);
    let master_clock = Arc::new(MasterClock::default());
    let mut driver = EngineIoDriver::new(engine, master_clock.clone(), None, MAX_BLOCK_FRAMES);

    let (capture_handle, capture_underruns) = match input_uid {
        Some(input_uid) => {
            let input_id = resolve_uid(&input_uid)?;
            let input_channels = channel_count(input_id, Direction::Input)
                .map_err(|e| format!("CoreAudio error {e}"))?
                .min(CHANNELS);
            if input_channels == 0 {
                return Err(format!("{input_uid} has no input channels"));
            }
            // Same PI gains and discontinuity threshold as loomix-soak's
            // proven values -- not retuned here.
            let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
            let attached = attach_capture_device(
                &mut driver,
                INPUT_STRIP,
                input_id,
                input_channels,
                master_clock.clone(),
                corrector,
                RING_CAPACITY,
            )
            .map_err(|e| format!("failed to start capture on {input_uid}: CoreAudio error {e}"))?;
            (Some(attached.io), Some(attached.dropouts))
        }
        None => (None, None),
    };

    let (sink, mut drain) = control::control_channel();
    let (mut control_pub, control_reader) = control::snapshot_channel(RECONCILE_QUEUE_CAPACITY);
    let (mut meter_pub, meter_reader) =
        control::latest_value_channel::<MeterSnapshot>(RECONCILE_QUEUE_CAPACITY);

    let callback: MasterTickCallback = Box::new(move |frames, input, output| {
        drain.drain_into(driver.engine_mut(), 64);
        driver.on_master_tick(frames as usize, input, output);
        control_pub.publish(ControlSnapshot::capture(driver.engine_mut()));
        meter_pub.publish(MeterSnapshot::capture(driver.engine_mut()));
    });
    let master = MasterIoProcHandle::start(output_id, callback)
        .map_err(|e| format!("failed to start output on {output_uid}: CoreAudio error {e}"))?;

    *state.session.lock().unwrap() = Some(AudioSession {
        sink,
        control_reader,
        meter_reader,
        capture_underruns,
        _capture: capture_handle,
        _master: master,
    });
    Ok(())
}

#[tauri::command]
fn disconnect_audio(state: State<AppState>) {
    *state.session.lock().unwrap() = None;
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            session: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            list_audio_devices,
            get_audio_status,
            connect_audio,
            disconnect_audio,
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
