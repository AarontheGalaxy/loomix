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
    self, BusSnapshot, CommandSink, ControlSnapshot, EngineCommand, EqSnapshot, LatestValueReader,
    MeterSnapshot, StripSnapshot, STRIP_EQ_CHANNELS,
};
use loomix_app::device_wiring::attach_capture_device;
use loomix_app::engine_io::{select_clock_master, DropoutCounter, EngineIoDriver};
use loomix_core::bus::BusMono;
use loomix_core::bus_mode::BusMode;
use loomix_core::intellipan::IntellipanMode;
use loomix_core::karaoke::KaraokeMode;
use loomix_core::parametric_eq::{EqCellParams, EqChannelParams, Memory};
use loomix_core::{Engine, CHANNELS, NUM_BUSES};
use loomix_hal::clock::{ClockSource, DeviceId};
use loomix_hal::device::{
    channel_count, device_name, device_uid, list_device_ids, nominal_sample_rate, transport_type,
    Direction, MasterTickCallback,
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

/// M11's resample-ratio sanity bound (`docs/SPEC.md`): a computed
/// `base_ratio` outside this range is refused rather than resampled.
/// This is Loomix's own choice, not a vendor-documented limit -- the
/// same "no published reference exists" category `docs/DSP.md` already
/// marks the macro-knob curves and Karaoke's mix depths with, checked the
/// same way (`docs/audit/`, no coverage found for this scenario in any
/// of the three vendor manuals, `docs/ARCHITECTURE.md`'s 2026-09-10
/// entry). Picked generously: 8kHz telephony-grade hardware against a
/// 192kHz interface is a 24x span, comfortably inside `[1/32, 32]`, so a
/// real, if unusual, device pairing is never refused just for being
/// uncommon -- only a non-finite, zero, or genuinely nonsensical rate
/// report trips it.
const MIN_BASE_RATIO: f32 = 1.0 / 32.0;
const MAX_BASE_RATIO: f32 = 32.0;

/// Everything a live audio connection owns: the two bridge halves the
/// Tauri commands below talk to, and the device handles that keep the
/// real I/O running -- dropping either handle stops and unregisters that
/// device (`loomix-hal`'s own `Drop` impls), which is exactly what
/// [`disconnect_audio`] relies on rather than any explicit teardown call.
struct AudioSession {
    sink: CommandSink,
    control_reader: LatestValueReader<ControlSnapshot>,
    meter_reader: LatestValueReader<MeterSnapshot>,
    eq_reader: LatestValueReader<EqSnapshot>,
    capture_underruns: Option<DropoutCounter>,
    /// M11: computed once at connect time from the actual connected
    /// devices' real channel counts/transport/rate -- see
    /// `output_device_warnings`/`input_device_warnings`.
    device_warnings: Vec<String>,
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

/// spec 1.18's "cycle Color, Position, Modulation."
fn intellipan_mode_to_str(mode: IntellipanMode) -> &'static str {
    match mode {
        IntellipanMode::Color => "color",
        IntellipanMode::Position => "position",
        IntellipanMode::Modulation => "modulation",
    }
}

fn intellipan_mode_from_str(s: &str) -> Option<IntellipanMode> {
    Some(match s {
        "color" => IntellipanMode::Color,
        "position" => IntellipanMode::Position,
        "modulation" => IntellipanMode::Modulation,
        _ => return None,
    })
}

/// spec 1.4's Karaoke button: off, K-m, K-1, K-2, K-v.
fn karaoke_mode_to_str(mode: KaraokeMode) -> &'static str {
    match mode {
        KaraokeMode::Off => "off",
        KaraokeMode::KM => "km",
        KaraokeMode::K1 => "k1",
        KaraokeMode::K2 => "k2",
        KaraokeMode::KV => "kv",
    }
}

fn karaoke_mode_from_str(s: &str) -> Option<KaraokeMode> {
    Some(match s {
        "off" => KaraokeMode::Off,
        "km" => KaraokeMode::KM,
        "k1" => KaraokeMode::K1,
        "k2" => KaraokeMode::K2,
        "kv" => KaraokeMode::KV,
        _ => return None,
    })
}

/// spec 1.7's A/B memory, shared by the strip and bus parametric EQ.
fn memory_to_str(memory: Memory) -> &'static str {
    match memory {
        Memory::A => "a",
        Memory::B => "b",
    }
}

fn memory_from_str(s: &str) -> Option<Memory> {
    Some(match s {
        "a" => Memory::A,
        "b" => Memory::B,
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
    /// `0.0` (center) on a virtual strip, which has no pan pot (spec 1.4
    /// has a 5.1 position pad instead) -- see `StripSnapshot::pan`.
    pan: f32,
    // M10: every field below mirrors `StripSnapshot`'s own doc comment --
    // a hardware-only or virtual-only field reads as that control's own
    // neutral default on the strip kind it doesn't apply to, same
    // convention as `pan` above.
    gate_knob: f32,
    comp_knob: f32,
    denoiser_knob: f32,
    limiter_threshold_db: f32,
    intellipan_mode: &'static str,
    intellipan_x: f32,
    intellipan_y: f32,
    strip_eq_on: bool,
    strip_eq_memory: &'static str,
    eq3_bass_db: f32,
    eq3_mid_db: f32,
    eq3_treble_db: f32,
    position_pad_x: f32,
    position_pad_y: f32,
    mc: bool,
    karaoke: &'static str,
}

impl From<StripSnapshot> for StripSnapshotDto {
    fn from(s: StripSnapshot) -> Self {
        Self {
            mute: s.mute,
            solo: s.solo,
            mono: s.mono,
            bus_assign: s.bus_assign,
            gain_layer_db: s.gain_layer_db,
            pan: s.pan,
            gate_knob: s.gate_knob,
            comp_knob: s.comp_knob,
            denoiser_knob: s.denoiser_knob,
            limiter_threshold_db: s.limiter_threshold_db,
            intellipan_mode: intellipan_mode_to_str(s.intellipan_mode),
            intellipan_x: s.intellipan_xy.0,
            intellipan_y: s.intellipan_xy.1,
            strip_eq_on: s.strip_eq_on,
            strip_eq_memory: memory_to_str(s.strip_eq_memory),
            eq3_bass_db: s.eq3_db.0,
            eq3_mid_db: s.eq3_db.1,
            eq3_treble_db: s.eq3_db.2,
            position_pad_x: s.position_pad.0,
            position_pad_y: s.position_pad.1,
            mc: s.mc,
            karaoke: karaoke_mode_to_str(s.karaoke),
        }
    }
}

#[derive(serde::Serialize)]
struct BusSnapshotDto {
    mute: bool,
    mono: &'static str,
    mode: &'static str,
    gain_db: f32,
    eq_on: bool,
    eq_memory: &'static str,
}

impl From<BusSnapshot> for BusSnapshotDto {
    fn from(b: BusSnapshot) -> Self {
        Self {
            mute: b.mute,
            mono: bus_mono_to_str(b.mono),
            mode: bus_mode_to_str(b.mode),
            gain_db: b.gain_db,
            eq_on: b.eq_on,
            eq_memory: memory_to_str(b.eq_memory),
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

/// M10's EQ panel state -- `EqChannelParams`/`EqCellParams` (`loomix-core`)
/// already derive `Serialize`/`Deserialize` for `loomix-config`'s on-disk
/// EQ file format, so this DTO reuses them directly rather than inventing
/// a parallel shape purely for IPC, unlike `BusMode`/`BusMono` above
/// (`docs/ARCHITECTURE.md`'s M8 entry): those don't derive serde at all,
/// so wrapping them in a string was the one, explicit translation point
/// instead of coupling `loomix-core`'s public enums to a wire format they
/// don't otherwise need -- that reasoning doesn't apply here, since the
/// coupling already exists for a real, independent reason.
#[derive(serde::Serialize)]
struct EqSnapshotDto {
    strips: Vec<[EqChannelParams; STRIP_EQ_CHANNELS]>,
    buses: Vec<[EqChannelParams; CHANNELS]>,
}

impl From<EqSnapshot> for EqSnapshotDto {
    fn from(s: EqSnapshot) -> Self {
        Self {
            strips: s.strips.to_vec(),
            buses: s.buses.to_vec(),
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
    /// M11: degraded-device warnings for this device, computed at
    /// picker-list time (`output_device_warnings`/`input_device_warnings`)
    /// so a degraded device can be spotted before connecting, not only
    /// after (`docs/SPEC.md`'s M11 entry -- "at device-selection time and
    /// on the connected-status line").
    warnings: Vec<String>,
}

#[derive(serde::Serialize)]
struct AudioStatusDto {
    connected: bool,
    /// `None` when connected with no input device attached, not just
    /// "zero so far" -- the UI needs to tell "no input selected" apart
    /// from "input selected, draining cleanly".
    capture_underruns: Option<u64>,
    /// M11: the same degraded-device warnings `list_audio_devices`
    /// already computes for the picker, recorded once at connect time for
    /// whichever devices actually got connected (`AudioSession::
    /// device_warnings`) -- the connected-status line's own copy, not a
    /// live re-query every poll.
    device_warnings: Vec<String>,
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
        let name = device_name(id).unwrap_or_default();
        let transport = transport_type(id).unwrap_or(0);
        let rate = nominal_sample_rate(id).unwrap_or(0.0);
        let mut warnings = output_device_warnings(&name, transport, rate, output_channels);
        warnings.extend(input_device_warnings(
            &name,
            transport,
            rate,
            input_channels,
        ));
        out.push(DeviceInfoDto {
            uid,
            name,
            input_channels,
            output_channels,
            warnings,
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
            device_warnings: s.device_warnings.clone(),
        },
        None => AudioStatusDto {
            connected: false,
            capture_underruns: None,
            device_warnings: Vec::new(),
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

/// M10: polled only while the EQ panel is actually open, not at
/// `get_control_snapshot`'s reconciliation rate -- see the `eq_pub`
/// publish site's own comment in `connect_audio`.
#[tauri::command]
fn get_eq_snapshot(state: State<AppState>) -> EqSnapshotDto {
    let mut session = state.session.lock().unwrap();
    match session.as_mut() {
        Some(s) => s.eq_reader.read().into(),
        None => EqSnapshot::default().into(),
    }
}

/// Hardware strips only (spec 1.2 step 7); a no-op on a virtual strip,
/// same as `EngineCommand::SetStripEqCell` itself.
#[tauri::command]
fn set_strip_eq_cell(
    state: State<AppState>,
    strip: usize,
    channel: usize,
    cell: usize,
    params: EqCellParams,
) {
    send(
        &state,
        EngineCommand::SetStripEqCell(strip, channel, cell, params),
    );
}

#[tauri::command]
fn set_bus_eq_cell(
    state: State<AppState>,
    bus: usize,
    channel: usize,
    cell: usize,
    params: EqCellParams,
) {
    send(
        &state,
        EngineCommand::SetBusEqCell(bus, channel, cell, params),
    );
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
fn set_strip_pan(state: State<AppState>, strip: usize, pan: f32) {
    send(&state, EngineCommand::SetStripPan(strip, pan));
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

// -- M10: the 2026-09-09 coverage audit's state-2 list, one Tauri command
// per `EngineCommand` variant added for it (`loomix-app::control`) -- the
// same trivial `send(&state, EngineCommand::X(...))` shape every command
// above already uses, cross-checked argument-for-argument against
// `control::tests::CONTROL_CASES`, the table that actually proves each
// variant's own round trip (`docs/ARCHITECTURE.md`'s M10 entry: this
// layer's own plumbing isn't separately unit-tested, same as every other
// trivial one-line command above -- the manual smoke test, spec 4.4,
// covers this layer once the UI drives it for real).

#[tauri::command]
fn set_strip_gate_knob(state: State<AppState>, strip: usize, knob: f32) {
    send(&state, EngineCommand::SetStripGateKnob(strip, knob));
}

#[tauri::command]
fn set_strip_comp_knob(state: State<AppState>, strip: usize, knob: f32) {
    send(&state, EngineCommand::SetStripCompKnob(strip, knob));
}

#[tauri::command]
fn set_strip_denoiser_knob(state: State<AppState>, strip: usize, knob: f32) {
    send(&state, EngineCommand::SetStripDenoiserKnob(strip, knob));
}

#[tauri::command]
fn set_strip_limiter_threshold(state: State<AppState>, strip: usize, db: f32) {
    send(&state, EngineCommand::SetStripLimiterThreshold(strip, db));
}

#[tauri::command]
fn set_strip_intellipan_mode(
    state: State<AppState>,
    strip: usize,
    mode: String,
) -> Result<(), String> {
    let mode = intellipan_mode_from_str(&mode)
        .ok_or_else(|| format!("unknown Intellipan mode: {mode}"))?;
    send(&state, EngineCommand::SetStripIntellipanMode(strip, mode));
    Ok(())
}

#[tauri::command]
fn set_strip_intellipan_xy(state: State<AppState>, strip: usize, x: f32, y: f32) {
    send(&state, EngineCommand::SetStripIntellipanXY(strip, x, y));
}

#[tauri::command]
fn set_strip_eq3(state: State<AppState>, strip: usize, bass_db: f32, mid_db: f32, treble_db: f32) {
    send(
        &state,
        EngineCommand::SetStripEq3(strip, bass_db, mid_db, treble_db),
    );
}

#[tauri::command]
fn set_strip_position_pad(state: State<AppState>, strip: usize, x: f32, y: f32) {
    send(&state, EngineCommand::SetStripPositionPad(strip, x, y));
}

#[tauri::command]
fn set_strip_mc(state: State<AppState>, strip: usize, on: bool) {
    send(&state, EngineCommand::SetStripMc(strip, on));
}

#[tauri::command]
fn set_strip_karaoke(state: State<AppState>, strip: usize, mode: String) -> Result<(), String> {
    let mode =
        karaoke_mode_from_str(&mode).ok_or_else(|| format!("unknown Karaoke mode: {mode}"))?;
    send(&state, EngineCommand::SetStripKaraoke(strip, mode));
    Ok(())
}

#[tauri::command]
fn set_strip_eq_on(state: State<AppState>, strip: usize, on: bool) {
    send(&state, EngineCommand::SetStripEqOn(strip, on));
}

#[tauri::command]
fn set_strip_eq_memory(state: State<AppState>, strip: usize, memory: String) -> Result<(), String> {
    let memory = memory_from_str(&memory).ok_or_else(|| format!("unknown EQ memory: {memory}"))?;
    send(&state, EngineCommand::SetStripEqMemory(strip, memory));
    Ok(())
}

#[tauri::command]
fn set_bus_eq_on(state: State<AppState>, bus: usize, on: bool) {
    send(&state, EngineCommand::SetBusEqOn(bus, on));
}

#[tauri::command]
fn set_bus_eq_memory(state: State<AppState>, bus: usize, memory: String) -> Result<(), String> {
    let memory = memory_from_str(&memory).ok_or_else(|| format!("unknown EQ memory: {memory}"))?;
    send(&state, EngineCommand::SetBusEqMemory(bus, memory));
    Ok(())
}

// -- M10 (continued): spec 1.7's per-channel trim/delay, FLAT and CH
// COPY -- brought into M10 rather than deferred (see `EngineCommand`'s own
// doc comment in `control.rs` for why).

#[tauri::command]
fn set_strip_eq_trim(state: State<AppState>, strip: usize, channel: usize, trim_db: f32) {
    send(
        &state,
        EngineCommand::SetStripEqTrim(strip, channel, trim_db),
    );
}

#[tauri::command]
fn set_strip_eq_delay(state: State<AppState>, strip: usize, channel: usize, delay_ms: f32) {
    send(
        &state,
        EngineCommand::SetStripEqDelay(strip, channel, delay_ms),
    );
}

#[tauri::command]
fn set_bus_eq_trim(state: State<AppState>, bus: usize, channel: usize, trim_db: f32) {
    send(&state, EngineCommand::SetBusEqTrim(bus, channel, trim_db));
}

#[tauri::command]
fn set_bus_eq_delay(state: State<AppState>, bus: usize, channel: usize, delay_ms: f32) {
    send(&state, EngineCommand::SetBusEqDelay(bus, channel, delay_ms));
}

#[tauri::command]
fn reset_strip_eq_channel(state: State<AppState>, strip: usize, channel: usize) {
    send(&state, EngineCommand::ResetStripEqChannel(strip, channel));
}

#[tauri::command]
fn reset_bus_eq_channel(state: State<AppState>, bus: usize, channel: usize) {
    send(&state, EngineCommand::ResetBusEqChannel(bus, channel));
}

#[tauri::command]
fn copy_strip_eq_channel(state: State<AppState>, strip: usize, from: usize, to: usize) {
    send(&state, EngineCommand::CopyStripEqChannel(strip, from, to));
}

#[tauri::command]
fn copy_bus_eq_channel(state: State<AppState>, bus: usize, from: usize, to: usize) {
    send(&state, EngineCommand::CopyBusEqChannel(bus, from, to));
}

/// Bluetooth's Hands-Free Profile (mono, telephony-grade codecs at
/// 8/16/24kHz) is CoreAudio's own well-known signature for "this
/// Bluetooth device's microphone is in use somewhere, which drops call
/// quality on *both* directions of the connection" -- confirmed against
/// real AirPods hardware (`transport` reads Bluetooth, `rate` reads
/// 24000 against A2DP's normal 44.1/48kHz, `docs/ARCHITECTURE.md`'s
/// 2026-09-10 entry). Loomix's own heuristic, not something CoreAudio
/// states directly or the vendor manuals cover at all (`docs/SPEC.md`'s
/// M11 entry, `docs/audit/`) -- generous enough to catch HFP's whole
/// documented rate range without ever flagging a genuine low-rate
/// non-Bluetooth device, since the transport check runs first and short-
/// circuits everything else.
const BLUETOOTH_REDUCED_PROFILE_RATE_CEILING: f64 = 32_000.0;

fn is_bluetooth_transport(transport: u32) -> bool {
    transport == loomix_hal::device::TRANSPORT_BLUETOOTH
        || transport == loomix_hal::device::TRANSPORT_BLUETOOTH_LE
}

/// Actionable, not diagnostic, per direct instruction: names what's
/// degraded (the real rate, in Bluetooth's reduced profile, not just "an
/// unusual number") and what would fix it (stop using the mic
/// elsewhere, reconnect) rather than only observing that something looks
/// off.
fn bluetooth_reduced_profile_warning(
    name: &str,
    transport: u32,
    rate: f64,
    channels: usize,
) -> Option<String> {
    if is_bluetooth_transport(transport)
        && rate > 0.0
        && rate < BLUETOOTH_REDUCED_PROFILE_RATE_CEILING
    {
        Some(format!(
            "{name} is running at {rate:.0} Hz, {channels}ch -- Bluetooth's reduced call-quality \
             profile, not its normal stereo quality. This usually means {name}'s microphone is in \
             use by this or another app; stop using it and reconnect {name} to restore full quality."
        ))
    } else {
        None
    }
}

/// Actionable, not diagnostic: names what's missing (no stereo field can
/// exist, not just "channel count looks low") and the fix (pick a
/// different device). Deliberately *not* a comparison against the bus's
/// own 8 channels (spec 1.1): the overwhelming majority of real output
/// devices are 2-channel, and the bus mode system (spec 1.6's Mix Down
/// A/B, Stereo Repeat, the Up Mix family) exists specifically to make
/// that the normal, unremarkable case -- warning on every ordinary
/// stereo speaker connection would be noise, not signal, and nothing a
/// user could act on (there's no "8-channel speaker" to go buy). A
/// single-channel output is the genuinely unusual case actually worth
/// naming: literally no stereo field can exist over it, regardless of
/// what the bus carries or how its mode is set -- exactly the AirPods
/// symptom this milestone started from ("a mono output path means the
/// stereo field cannot exist at all regardless of what the source is,"
/// per the session that found it). Output-only: a mono *input* device is
/// completely ordinary (most real microphones are mono), so this has no
/// input-side equivalent.
fn mono_output_warning(name: &str, channels: usize) -> Option<String> {
    if channels == 1 {
        Some(format!(
            "{name} only provides 1 output channel, so no stereo field can exist over it \
             regardless of the mix -- choose a different device for stereo output."
        ))
    } else {
        None
    }
}

/// Every warning that applies to `name` as an *output* device, in
/// priority order and deduplicated: when the Bluetooth-reduced-profile
/// explanation applies, it already accounts for the mono output too (its
/// own message names the channel count), so the more generic mono
/// warning is skipped rather than shown alongside it -- one clear cause,
/// not two messages describing the same symptom from different angles.
fn output_device_warnings(name: &str, transport: u32, rate: f64, channels: usize) -> Vec<String> {
    if let Some(w) = bluetooth_reduced_profile_warning(name, transport, rate, channels) {
        vec![w]
    } else if let Some(w) = mono_output_warning(name, channels) {
        vec![w]
    } else {
        Vec::new()
    }
}

/// The input-side mirror of [`output_device_warnings`]: only the
/// Bluetooth-reduced-profile check applies (see [`mono_output_warning`]'s
/// own doc comment for why a low channel count isn't a warning on this
/// side).
fn input_device_warnings(name: &str, transport: u32, rate: f64, channels: usize) -> Vec<String> {
    bluetooth_reduced_profile_warning(name, transport, rate, channels)
        .into_iter()
        .collect()
}

/// The fixed ratio a capture device's real nominal rate needs against the
/// master's (spec 2.3's "feed the ratio into a polyphase resampler," now
/// computed directly instead of left for the drift corrector to
/// discover -- `docs/ARCHITECTURE.md`'s 2026-09-10 entry). A plain
/// division of two already-queried rates, refused rather than resampled
/// if it's not finite, not positive, or outside `[MIN_BASE_RATIO,
/// MAX_BASE_RATIO]`.
fn base_ratio_for(master_rate: f64, device_rate: f64) -> Result<f32, String> {
    let ratio = (master_rate / device_rate) as f32;
    if !ratio.is_finite() || ratio <= 0.0 {
        return Err(format!(
            "reported a nominal rate of {device_rate} Hz against a {master_rate} Hz master, \
             which CoreAudio can't give a usable resample ratio for"
        ));
    }
    if !(MIN_BASE_RATIO..=MAX_BASE_RATIO).contains(&ratio) {
        return Err(format!(
            "its {device_rate} Hz nominal rate against a {master_rate} Hz master needs a \
             {ratio:.3}x resample ratio, outside the {MIN_BASE_RATIO:.4}x..{MAX_BASE_RATIO}x \
             range Loomix will attempt"
        ));
    }
    Ok(ratio)
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
    let output_name = device_name(output_id).unwrap_or_default();
    // M11: computed once here, from the device actually being connected --
    // see `output_device_warnings`'s own doc comment for what's checked
    // and why a plain "fewer than 8 channels" isn't one of them.
    let mut device_warnings = output_device_warnings(
        &output_name,
        transport_type(output_id).unwrap_or(0),
        sample_rate,
        output_channels,
    );

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
            // M11: the other half of the 2026-08-28 gap -- only the
            // output device's nominal rate was ever read before this.
            // `base_ratio` is the fixed, known quantity a genuine
            // mismatch needs, computed once here rather than left for
            // the drift corrector to discover (it can't -- see
            // `docs/ARCHITECTURE.md`'s 2026-09-10 entry).
            let input_rate =
                nominal_sample_rate(input_id).map_err(|e| format!("CoreAudio error {e}"))?;
            let base_ratio =
                base_ratio_for(sample_rate, input_rate).map_err(|e| format!("{input_uid}: {e}"))?;
            let input_name = device_name(input_id).unwrap_or_default();
            device_warnings.extend(input_device_warnings(
                &input_name,
                transport_type(input_id).unwrap_or(0),
                input_rate,
                input_channels,
            ));
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
                base_ratio,
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
    // M10: the EQ panel polls this only while actually open, not at the
    // reconciliation rate the other two get -- still published every
    // callback like the others (a fixed-size array copy, no allocation,
    // spec 3.3), just read less often.
    let (mut eq_pub, eq_reader) = control::eq_snapshot_channel(RECONCILE_QUEUE_CAPACITY);

    let callback: MasterTickCallback = Box::new(move |frames, input, output| {
        drain.drain_into(driver.engine_mut(), 64);
        driver.on_master_tick(frames as usize, input, output);
        control_pub.publish(ControlSnapshot::capture(driver.engine_mut()));
        meter_pub.publish(MeterSnapshot::capture(driver.engine_mut()));
        eq_pub.publish(EqSnapshot::capture(driver.engine_mut()));
    });
    let master = MasterIoProcHandle::start(output_id, callback)
        .map_err(|e| format!("failed to start output on {output_uid}: CoreAudio error {e}"))?;

    *state.session.lock().unwrap() = Some(AudioSession {
        sink,
        control_reader,
        meter_reader,
        eq_reader,
        capture_underruns,
        device_warnings,
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
            set_strip_pan,
            set_bus_mute,
            set_bus_mono,
            set_bus_mode,
            set_bus_gain,
            set_strip_gate_knob,
            set_strip_comp_knob,
            set_strip_denoiser_knob,
            set_strip_limiter_threshold,
            set_strip_intellipan_mode,
            set_strip_intellipan_xy,
            set_strip_eq3,
            set_strip_position_pad,
            set_strip_mc,
            set_strip_karaoke,
            set_strip_eq_on,
            set_strip_eq_memory,
            set_bus_eq_on,
            set_bus_eq_memory,
            get_eq_snapshot,
            set_strip_eq_cell,
            set_bus_eq_cell,
            set_strip_eq_trim,
            set_strip_eq_delay,
            set_bus_eq_trim,
            set_bus_eq_delay,
            reset_strip_eq_channel,
            reset_bus_eq_channel,
            copy_strip_eq_channel,
            copy_bus_eq_channel,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Loomix app");
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomix_hal::device::{TRANSPORT_BLUETOOTH, TRANSPORT_BLUETOOTH_LE};

    const BUILTIN_TRANSPORT: u32 = 0x626c_746e; // 'bltn', an arbitrary non-Bluetooth transport for these tests

    #[test]
    fn bluetooth_reduced_profile_warning_fires_for_the_real_airpods_signature() {
        // transport=blue, rate=24000, 1ch -- exactly what real AirPods
        // reported (`docs/ARCHITECTURE.md`'s 2026-09-10 entry).
        let warning =
            bluetooth_reduced_profile_warning("Eren (AirPods)", TRANSPORT_BLUETOOTH, 24_000.0, 1);
        let warning = warning.expect("the real AirPods signature should produce a warning");
        assert!(
            warning.contains("24000") && warning.contains("Eren (AirPods)"),
            "should name the device and the actual degraded rate, not just say something's wrong: {warning}"
        );
        assert!(
            warning.to_lowercase().contains("microphone")
                || warning.to_lowercase().contains("reconnect"),
            "should say what would fix it, not just diagnose it (direct instruction): {warning}"
        );
    }

    #[test]
    fn bluetooth_reduced_profile_warning_also_fires_for_bluetooth_le() {
        assert!(bluetooth_reduced_profile_warning(
            "Some LE headset",
            TRANSPORT_BLUETOOTH_LE,
            16_000.0,
            1
        )
        .is_some());
    }

    #[test]
    fn bluetooth_reduced_profile_warning_does_not_fire_for_a_normal_bluetooth_rate() {
        // The same transport, but a real A2DP stereo connection (44.1kHz) --
        // must not be flagged just for being Bluetooth.
        assert!(
            bluetooth_reduced_profile_warning("Eren (AirPods)", TRANSPORT_BLUETOOTH, 44_100.0, 2)
                .is_none(),
            "a normal A2DP-quality Bluetooth connection should never be flagged"
        );
    }

    #[test]
    fn bluetooth_reduced_profile_warning_does_not_fire_for_a_non_bluetooth_low_rate_device() {
        // A genuinely low-rate wired/built-in device (old telephony
        // hardware, a cheap USB mic) is not "reduced" -- it's just what
        // it is. The transport check must gate this, not the rate alone.
        assert!(
            bluetooth_reduced_profile_warning("Old USB Headset", BUILTIN_TRANSPORT, 8_000.0, 1)
                .is_none(),
            "a low rate on a non-Bluetooth device should never be flagged as a reduced profile"
        );
    }

    #[test]
    fn mono_output_warning_fires_and_is_actionable() {
        let warning = mono_output_warning("Eren (AirPods)", 1);
        let warning = warning.expect("a mono output should warn");
        assert!(
            warning.to_lowercase().contains("stereo"),
            "should name what's actually missing (a stereo field), not just an unusual number: {warning}"
        );
        assert!(
            warning.to_lowercase().contains("choose")
                || warning.to_lowercase().contains("different"),
            "should say what would fix it, not just that it's mono: {warning}"
        );
    }

    #[test]
    fn mono_output_warning_does_not_fire_for_an_ordinary_stereo_device() {
        // The case this test exists to pin down: a ubiquitous, completely
        // healthy 2-channel output (built-in speakers, headphones) must
        // never be flagged just for being short of the bus's own 8
        // channels -- that's what bus modes (spec 1.6) are for.
        assert!(mono_output_warning("MacBook Pro Hoparlörü", 2).is_none());
    }

    #[test]
    fn mono_output_warning_does_not_fire_at_the_bus_s_own_full_channel_count() {
        assert!(mono_output_warning("Full Interface", CHANNELS).is_none());
    }

    #[test]
    fn mono_output_warning_does_not_fire_at_zero_channels() {
        // A device offering zero output channels is rejected earlier in
        // `connect_audio` with its own distinct error -- not this warning's job.
        assert!(mono_output_warning("No Output", 0).is_none());
    }

    #[test]
    fn output_device_warnings_prefers_the_bluetooth_explanation_over_the_generic_one() {
        // The exact real AirPods case: both conditions are technically
        // true (1ch, and it's a reduced Bluetooth profile), but only the
        // more specific, causal explanation should surface -- not both,
        // which would read as two different problems instead of one.
        let warnings = output_device_warnings("Eren (AirPods)", TRANSPORT_BLUETOOTH, 24_000.0, 1);
        assert_eq!(
            warnings.len(),
            1,
            "exactly one warning, not both: {warnings:?}"
        );
        assert!(
            warnings[0].to_lowercase().contains("bluetooth"),
            "the Bluetooth explanation should win, got: {warnings:?}"
        );
    }

    #[test]
    fn output_device_warnings_falls_back_to_the_generic_one_for_a_non_bluetooth_shortfall() {
        let warnings =
            output_device_warnings("Cheap USB Interface", BUILTIN_TRANSPORT, 44_100.0, 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("output channel"));
    }

    #[test]
    fn output_device_warnings_is_empty_for_a_healthy_device() {
        assert!(
            output_device_warnings("MacBook Pro Hoparlörü", BUILTIN_TRANSPORT, 44_100.0, 2)
                .is_empty()
        );
    }

    #[test]
    fn input_device_warnings_only_ever_checks_the_bluetooth_case() {
        // A mono microphone is completely normal on the input side --
        // must never be flagged, unlike the output-side channel check.
        assert!(input_device_warnings("Normal Mic", BUILTIN_TRANSPORT, 44_100.0, 1).is_empty());
        assert_eq!(
            input_device_warnings("Eren (AirPods)", TRANSPORT_BLUETOOTH, 24_000.0, 1).len(),
            1
        );
    }

    #[test]
    fn base_ratio_for_a_real_airpods_pairing_matches_the_hand_derived_value() {
        // 44100 Hz master, 24000 Hz AirPods HFP capture (confirmed live
        // against real hardware, `docs/ARCHITECTURE.md`'s 2026-09-10
        // entry) -- well inside the accepted range.
        let ratio = base_ratio_for(44_100.0, 24_000.0).expect("a real pairing should be accepted");
        assert!((ratio - 44_100.0 / 24_000.0).abs() < 1e-6, "got {ratio}");
    }

    #[test]
    fn base_ratio_for_the_same_nominal_rate_is_exactly_one() {
        let ratio =
            base_ratio_for(48_000.0, 48_000.0).expect("same-rate pairing should be accepted");
        assert_eq!(
            ratio, 1.0,
            "no genuine mismatch should give a ratio of exactly 1.0"
        );
    }

    #[test]
    fn base_ratio_for_a_zero_device_rate_is_refused() {
        assert!(
            base_ratio_for(48_000.0, 0.0).is_err(),
            "a zero nominal rate can't give a usable ratio"
        );
    }

    #[test]
    fn base_ratio_for_an_extreme_mismatch_is_refused() {
        // 192kHz master against an 8kHz device is a real, if extreme,
        // pairing -- 24x, inside the bound -- but past MAX_BASE_RATIO
        // (32x) something has gone wrong with the reported rates, not a
        // real device this codebase should attempt to resample across.
        assert!(
            base_ratio_for(192_000.0, 1_000.0).is_err(),
            "a 192x mismatch should be refused, not resampled"
        );
        assert!(
            base_ratio_for(1_000.0, 192_000.0).is_err(),
            "the same extreme mismatch in the other direction should also be refused"
        );
    }

    #[test]
    fn base_ratio_for_stays_within_bounds_at_the_edges_of_a_realistic_range() {
        // 8kHz telephony-grade hardware against a 192kHz interface (a
        // 24x span, `docs/SPEC.md`'s own worked example for why the
        // bound is generous) should still be accepted in both
        // directions.
        assert!(base_ratio_for(192_000.0, 8_000.0).is_ok());
        assert!(base_ratio_for(8_000.0, 192_000.0).is_ok());
    }
}
